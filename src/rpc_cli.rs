use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use agentx::{
  Config, EventSink, Session, SessionStore,
  protocol::{Image, ToolCall},
  run_control,
  tool::Risk,
};

use super::app::make_agent_with;

#[derive(Deserialize)]
struct Request {
  #[serde(default)]
  id: Value,
  method: String,
  #[serde(default)]
  params: Value,
}

pub async fn run(workspace: PathBuf, config: Config, no_save: bool) -> Result<()> {
  let events = EventSink::new(|event| {
    println!(
      "{}",
      json!({"jsonrpc": "2.0", "method": "event", "params": event})
    );
  });
  let approver = Arc::new(|_: &ToolCall, _: Risk| false) as agentx::agent::Approver;
  let (agent, options) = make_agent_with(&workspace, &config, events, approver).await?;
  let store = SessionStore::default_store()?;
  let mut session = Session::new(workspace);
  let mut lines = BufReader::new(tokio::io::stdin()).lines();

  while let Some(line) = lines.next_line().await? {
    let request = match serde_json::from_str::<Request>(&line) {
      Ok(request) => request,
      Err(error) => {
        respond_error(Value::Null, -32700, &format!("parse error: {error}"));
        continue;
      }
    };
    match request.method.as_str() {
      "prompt" => {
        let Some(prompt) = request
          .params
          .get("prompt")
          .and_then(Value::as_str)
          .map(str::to_string)
        else {
          respond_error(request.id, -32602, "prompt is required");
          continue;
        };
        let images = match load_images(&request.params).await {
          Ok(images) => images,
          Err(error) => {
            respond_error(request.id, -32602, &format!("{error:#}"));
            continue;
          }
        };
        let outcome = {
          let (controller, mut inbox) = run_control();
          let execution = async {
            if images.is_empty() {
              agent
                .run_controlled(&mut session, prompt, options.clone(), &mut inbox)
                .await
            } else {
              agent
                .run_controlled_with_images(
                  &mut session,
                  prompt,
                  images,
                  options.clone(),
                  &mut inbox,
                )
                .await
            }
          };
          tokio::pin!(execution);
          let mut input_open = true;
          loop {
            tokio::select! {
              result = &mut execution => break result,
              line = lines.next_line(), if input_open => {
                match line? {
                  Some(line) => handle_active(&line, &controller),
                  None => {
                    input_open = false;
                    controller.cancel();
                  }
                }
              }
            }
          }
        };
        if !no_save {
          store.save(&session).await?;
        }
        match outcome {
          Ok(output) => respond(
            request.id,
            json!({
              "output": output, "session_id": session.id, "usage": session.usage,
            }),
          ),
          Err(error) => respond_error(request.id, -32000, &format!("{error:#}")),
        }
      }
      "state" => respond(request.id, serde_json::to_value(&session)?),
      "new_session" => {
        session = Session::new(session.workspace.clone());
        respond(request.id, json!({"session_id": session.id}));
      }
      "save" => {
        store.save(&session).await?;
        respond(request.id, json!({"saved": true}));
      }
      "shutdown" => {
        respond(request.id, json!({"shutdown": true}));
        break;
      }
      "steer" | "cancel" => {
        respond_error(request.id, -32001, "no run is active");
      }
      _ => respond_error(request.id, -32601, "method not found"),
    }
  }
  Ok(())
}

fn handle_active(line: &str, controller: &agentx::RunController) {
  let request = match serde_json::from_str::<Request>(line) {
    Ok(request) => request,
    Err(error) => {
      respond_error(Value::Null, -32700, &format!("parse error: {error}"));
      return;
    }
  };
  match request.method.as_str() {
    "steer" => {
      let message = request.params.get("message").and_then(Value::as_str);
      match message {
        Some(message) if controller.steer(message) => {
          respond(request.id, json!({"queued": true}));
        }
        Some(_) => respond_error(request.id, -32002, "run control is closed"),
        None => respond_error(request.id, -32602, "message is required"),
      }
    }
    "cancel" => {
      if controller.cancel() {
        respond(request.id, json!({"cancelled": true}));
      } else {
        respond_error(request.id, -32002, "run control is closed");
      }
    }
    _ => respond_error(request.id, -32003, "a run is already active"),
  }
}

async fn load_images(params: &Value) -> Result<Vec<Image>> {
  let paths = params.get("images").map_or(Ok(&[][..]), |value| {
    value
      .as_array()
      .map(Vec::as_slice)
      .context("images must be an array")
  })?;
  let mut images = Vec::with_capacity(paths.len());
  for path in paths {
    let path = path.as_str().context("image path must be a string")?;
    images.push(Image::from_path(&PathBuf::from(path)).await?);
  }
  Ok(images)
}

fn respond(id: Value, result: Value) {
  println!("{}", json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn respond_error(id: Value, code: i64, message: &str) {
  println!(
    "{}",
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
  );
}
