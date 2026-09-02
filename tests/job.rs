use std::time::Duration;

use agentx::{
  JobStore,
  tool::{Risk, ToolContext},
};
use serde_json::{Value, json};

#[tokio::test]
async fn background_jobs_persist_status_and_output() {
  let temp = tempfile::tempdir().unwrap();
  let tool = JobStore::new(temp.path().join("jobs")).tool();
  let context = ToolContext {
    workspace: temp.path().to_path_buf(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };
  assert_eq!(tool.risk(&json!({"command": "start"})), Risk::Execute);
  let started: Value = serde_json::from_str(
    &tool
      .execute(
        &context,
        json!({"command": "start", "shell": "printf 'durable output'; exit 7"}),
      )
      .await
      .unwrap(),
  )
  .unwrap();
  let id = started["id"].as_str().unwrap();

  let mut status = Value::Null;
  for _ in 0..100 {
    status = serde_json::from_str(
      &tool
        .execute(&context, json!({"command": "status", "id": id}))
        .await
        .unwrap(),
    )
    .unwrap();
    if status["state"] == "exited" {
      break;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  assert_eq!(status["exit_code"], 7);
  let output = tool
    .execute(&context, json!({"command": "output", "id": id}))
    .await
    .unwrap();
  assert_eq!(output, "durable output");

  let listed: Value = serde_json::from_str(
    &tool
      .execute(&context, json!({"command": "list"}))
      .await
      .unwrap(),
  )
  .unwrap();
  assert_eq!(listed.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn background_jobs_can_be_stopped_safely() {
  let temp = tempfile::tempdir().unwrap();
  let tool = JobStore::new(temp.path().join("jobs")).tool();
  let context = ToolContext {
    workspace: temp.path().to_path_buf(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 4096,
  };
  let started: Value = serde_json::from_str(
    &tool
      .execute(&context, json!({"command": "start", "shell": "sleep 30"}))
      .await
      .unwrap(),
  )
  .unwrap();
  let id = started["id"].as_str().unwrap();
  let stopped: Value = serde_json::from_str(
    &tool
      .execute(&context, json!({"command": "stop", "id": id}))
      .await
      .unwrap(),
  )
  .unwrap();
  assert_eq!(stopped["state"], "stopped");
}

#[tokio::test]
async fn long_job_output_keeps_its_tail() {
  let temp = tempfile::tempdir().unwrap();
  let tool = JobStore::new(temp.path().join("jobs")).tool();
  let context = ToolContext {
    workspace: temp.path().to_path_buf(),
    session_id: uuid::Uuid::nil(),
    max_output_bytes: 256,
  };
  let started: Value = serde_json::from_str(
    &tool
      .execute(
        &context,
        json!({"command": "start", "shell": "i=0; while [ $i -lt 100 ]; do echo line $i; i=$((i+1)); done"}),
      )
      .await
      .unwrap(),
  )
  .unwrap();
  let id = started["id"].as_str().unwrap();
  for _ in 0..100 {
    let status: Value = serde_json::from_str(
      &tool
        .execute(&context, json!({"command": "status", "id": id}))
        .await
        .unwrap(),
    )
    .unwrap();
    if status["state"] == "exited" {
      break;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
  }
  let output = tool
    .execute(&context, json!({"command": "output", "id": id}))
    .await
    .unwrap();
  assert!(output.starts_with("[earlier output omitted]\n"));
  assert!(output.trim_end().ends_with("line 99"));
  assert!(!output.contains("line 0\n"));
}
