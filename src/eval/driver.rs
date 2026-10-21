use std::{
  path::PathBuf,
  sync::{Arc, Mutex},
  time::Duration,
};

// dependencies
use ainz::{
  Agent, Config, Event, EventSink, RunOptions, RuntimeProvider, Session, SkillCatalog, deny_all,
  instruction,
  protocol::{Message, Role, ToolCall},
  skill::Skill,
  tool::{ToolSet, builtins},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
  let request: Value = serde_json::from_slice(
    &tokio::fs::read(
      std::env::args_os()
        .nth(1)
        .context("request path is required")?,
    )
    .await?,
  )?;
  let config: Config = serde_json::from_value(request["config"].clone())?;
  let workspace = PathBuf::from(
    request["workspace"]
      .as_str()
      .context("workspace is required")?,
  );
  let provider = RuntimeProvider::configured(&config, &workspace).await?;
  let mut tools = ToolSet::default();
  tools.extend(builtins())?;
  let mut catalog = SkillCatalog::default();
  for (index, skill) in request["skills"]
    .as_array()
    .context("frozen skills are required")?
    .iter()
    .enumerate()
  {
    catalog.skills.push(Skill {
      name: skill["name"]
        .as_str()
        .context("skill name is required")?
        .into(),
      description: skill["description"].as_str().unwrap_or_default().into(),
      path: workspace
        .join(".ainz/skills")
        .join(index.to_string())
        .join("SKILL.md"),
    });
  }
  tools.insert(catalog.tool())?;
  let allowed = vec![
    "read".into(),
    "list".into(),
    "search".into(),
    "write".into(),
    "edit".into(),
    "shell".into(),
    "skill".into(),
  ];
  tools = tools.restricted(Some(&allowed), None);
  let observed = Arc::new(Mutex::new(Vec::<ToolCall>::new()));
  let events = EventSink::new({
    let observed = observed.clone();
    move |event| {
      if let Event::ToolStart { call } = event {
        observed.lock().unwrap().push(call);
      }
    }
  });
  let mut session = Session::new(workspace.clone());
  if let Some(skills) = request["loaded_skills"].as_array() {
    for (index, skill) in skills.iter().enumerate() {
      let id = format!("loaded{index}");
      let mut call = Message::text(
        Role::Assistant,
        "Loading a proposed procedure for this task.",
      );
      call.tool_calls.push(ToolCall {
        id: id.clone(),
        name: "skill".into(),
        arguments: json!({"name":skill["name"]}),
      });
      session.append(call);
      session.append(Message::tool(
        id,
        skill["text"].as_str().unwrap_or_default(),
      ));
    }
  }
  let options = RunOptions {
    instructions: instruction::load(&workspace).await?,
    hooks: Default::default(),
    ..RunOptions::from(&config)
  };
  let agent = Agent::new(provider, tools, workspace, events, deny_all());
  // cancel inside the driver first, so tool futures can clean up their process groups
  let result = tokio::time::timeout(
    Duration::from_secs(100),
    agent.run(
      &mut session,
      request["prompt"]
        .as_str()
        .context("prompt is required")?
        .into(),
      options,
    ),
  )
  .await
  .context("behavioral task timed out")
  .and_then(|result| result);
  let completed = result.is_ok()
    && session
      .messages()?
      .last()
      .is_some_and(|m| m.role == Role::Assistant && m.tool_calls.is_empty());
  let (output, error) = match result {
    Ok(text) => (Some(text), None),
    Err(error) => (None, Some(format!("{error:#}"))),
  };
  println!(
    "{}",
    json!({"completed":completed, "output":output, "error":error,
    "usage":session.usage, "calls":*observed.lock().unwrap()})
  );
  Ok(())
}
