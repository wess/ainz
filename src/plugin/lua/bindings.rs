use std::{
  collections::BTreeMap,
  sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
  },
};

use mlua::{Function, Lua, LuaSerdeExt, Value, chunk::ChunkMode};
use tokio::sync::Mutex;

use super::bundle::Bundle;
use crate::{plugin::host::Host, tool::ToolContext};

pub(super) fn install(
  lua: &Lua,
  host: Arc<Mutex<Host>>,
  context: &ToolContext,
) -> mlua::Result<()> {
  let api = lua.create_table()?;
  let read = host.clone();
  api.set(
    "read",
    lua.create_async_function(move |_, path: String| {
      let host = read.clone();
      async move {
        host
          .lock()
          .await
          .host_read(&path)
          .await
          .map_err(mlua::Error::external)
      }
    })?,
  )?;
  let write = host.clone();
  api.set(
    "write",
    lua.create_async_function(move |_, (path, content): (String, String)| {
      let host = write.clone();
      async move {
        host
          .lock()
          .await
          .host_write(&path, &content)
          .await
          .map_err(mlua::Error::external)
      }
    })?,
  )?;
  let run = host.clone();
  api.set(
    "run",
    lua.create_async_function(move |_, command: String| {
      let host = run.clone();
      async move {
        host
          .lock()
          .await
          .host_run(&command)
          .await
          .map_err(mlua::Error::external)
      }
    })?,
  )?;
  api.set(
    "fetch",
    lua.create_async_function(move |_, url: String| {
      let host = host.clone();
      async move {
        host
          .lock()
          .await
          .host_fetch(&url)
          .await
          .map_err(mlua::Error::external)
      }
    })?,
  )?;
  let context = context.clone();
  let reported = Arc::new(AtomicUsize::new(0));
  api.set(
    "log",
    lua.create_function(move |_, text: String| {
      let before = reported.fetch_add(text.len().min(context.max_output_bytes), Ordering::Relaxed);
      let remaining = context.max_output_bytes.saturating_sub(before);
      if remaining > 0 {
        context.report(&crate::tool::truncate(text, remaining));
      }
      Ok(())
    })?,
  )?;
  api.set(
    "json_encode",
    lua.create_function(|lua, value: Value| {
      serde_json::to_string(&lua.from_value::<serde_json::Value>(value)?)
        .map_err(mlua::Error::external)
    })?,
  )?;
  api.set(
    "json_decode",
    lua.create_function(|lua, text: String| {
      lua
        .to_value(&serde_json::from_str::<serde_json::Value>(&text).map_err(mlua::Error::external)?)
    })?,
  )?;
  lua.globals().set("ainz", api)
}

pub(super) fn modules(lua: &Lua, bundle: Arc<Bundle>) -> mlua::Result<()> {
  let loaded = lua.create_table()?;
  let loading = Arc::new(std::sync::Mutex::new(BTreeMap::<String, bool>::new()));
  lua.globals().set(
    "require",
    lua.create_async_function(move |lua, name: String| {
      let bundle = bundle.clone();
      let loaded = loaded.clone();
      let loading = loading.clone();
      async move {
        if name.is_empty()
          || name.split('.').any(|part| {
            part.is_empty()
              || !part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
          })
        {
          return Err(mlua::Error::runtime("invalid module name"));
        }
        let cached: Value = loaded.get(name.as_str())?;
        if !cached.is_nil() {
          return Ok(cached);
        }
        {
          let mut active = loading
            .lock()
            .map_err(|error| mlua::Error::runtime(error.to_string()))?;
          if active.insert(name.clone(), true).is_some() {
            return Err(mlua::Error::runtime("circular module dependency"));
          }
        }
        let path = name.replace('.', "/");
        let result = async {
          let candidates = [format!("lua/{path}.lua"), format!("lua/{path}/init.lua")];
          let (path, bytes) = candidates
            .iter()
            .find_map(|path| bundle.files.get(path).map(|bytes| (path, bytes)))
            .ok_or_else(|| mlua::Error::runtime(format!("bundled module {name} was not found")))?;
          let function: Function = lua
            .load(bytes)
            .set_name(path)
            .set_mode(ChunkMode::Text)
            .into_function()?;
          let value: Value = function.call_async(()).await?;
          let value = if value.is_nil() {
            Value::Boolean(true)
          } else {
            value
          };
          loaded.set(name.as_str(), value.clone())?;
          Ok(value)
        }
        .await;
        loading
          .lock()
          .map_err(|error| mlua::Error::runtime(error.to_string()))?
          .remove(&name);
        result
      }
    })?,
  )
}
