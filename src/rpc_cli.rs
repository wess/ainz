use std::path::PathBuf;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::{
  io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter},
  sync::mpsc,
  task::JoinHandle,
};

use ainz::{Config, EventSink, Session, SessionStore, deny_all, protocol::Image, run_control};

use super::app::make_agent_parts;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const NO_SUCH_METHOD: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const RUN_FAILED: i64 = -32000;
const NO_RUN: i64 = -32001;
const CONTROL_CLOSED: i64 = -32002;
const RUN_ACTIVE: i64 = -32003;

// a request is a line, and a line has to end somewhere: past this the rest of it is thrown
// away rather than held, and the request is answered as unparseable
const MAX_LINE: usize = 16 * 1024 * 1024;

#[derive(Debug)]
struct Request {
  // absent is a notification, which JSON-RPC answers by not answering; null is still an id
  id: Option<Value>,
  method: String,
  params: Value,
}

pub async fn run(workspace: PathBuf, config: Config, no_save: bool) -> Result<()> {
  let (out, writer) = writer();
  let events = EventSink::new({
    let out = out.clone();
    move |event| out.send(&json!({"jsonrpc": "2.0", "method": "event", "params": event}))
  });
  let served = match make_agent_parts(&workspace, &config, events, deny_all()).await {
    Ok((agent, options, mcp)) => {
      let served = async {
        let store = SessionStore::default_store()?;
        let mut session = Session::new(workspace);
        let mut input = Input::new(tokio::io::stdin());
        while let Some(line) = input.next().await? {
          let request = match line.as_deref().map(parse) {
            Some(Ok(request)) => request,
            Some(Err(rejected)) => {
              out.send(&rejected);
              continue;
            }
            None => {
              out.send(&failure(
                Value::Null,
                PARSE_ERROR,
                "request line is too long",
              ));
              continue;
            }
          };
          let Request { id, method, params } = request;
          match method.as_str() {
            "prompt" => {
              let Some(prompt) = params
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_string)
              else {
                out.fail(id, INVALID_PARAMS, "prompt is required");
                continue;
              };
              let images = match load_images(&params).await {
                Ok(images) => images,
                Err(error) => {
                  out.fail(id, INVALID_PARAMS, &format!("{error:#}"));
                  continue;
                }
              };
              let mut stopping = false;
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
                    // nothing more is read once shutdown is asked for: a read left waiting on
                    // stdin would hold the runtime open after the server is done
                    line = input.next(), if input_open && !stopping => match line? {
                      Some(line) => stopping |= handle_active(line.as_deref(), &controller, &out),
                      None => {
                        input_open = false;
                        controller.cancel();
                      }
                    },
                  }
                }
              };
              // a transcript that could not be written is reported with the run, not a
              // reason to stop serving
              let saved = if no_save {
                Ok(())
              } else {
                store.save(&session).await
              };
              match outcome {
                Ok(output) => {
                  let mut result = json!({
                    "output": output, "session_id": session.id, "usage": session.usage,
                  });
                  if let Err(error) = saved {
                    result["save_error"] = json!(format!("{error:#}"));
                  }
                  out.reply(id, result);
                }
                Err(error) => {
                  let mut message = format!("{error:#}");
                  if let Err(error) = saved {
                    message.push_str(&format!("; the session was not saved: {error:#}"));
                  }
                  out.fail(id, RUN_FAILED, &message);
                }
              }
              if stopping {
                break;
              }
            }
            "state" => match serde_json::to_value(&session) {
              Ok(state) => out.reply(id, state),
              Err(error) => out.fail(id, RUN_FAILED, &error.to_string()),
            },
            "new_session" => {
              session = Session::new(session.workspace.clone());
              out.reply(id, json!({"session_id": session.id}));
            }
            "save" => match store.save(&session).await {
              Ok(()) => out.reply(id, json!({"saved": true})),
              Err(error) => out.fail(id, RUN_FAILED, &format!("save failed: {error:#}")),
            },
            "shutdown" => {
              out.reply(id, json!({"shutdown": true}));
              break;
            }
            "steer" | "cancel" => out.fail(id, NO_RUN, "no run is active"),
            _ => out.fail(id, NO_SUCH_METHOD, "method not found"),
          }
        }
        anyhow::Ok(())
      }
      .await;
      mcp.shutdown();
      served
    }
    Err(error) => Err(error),
  };
  // everything queued before the close, the reply to shutdown included, reaches stdout before
  // the process leaves
  out.close();
  drop(writer.await);
  served
}

// what a request arriving mid-run may do to it; anything else waits for the run to end.
// the answer is whether the request asked the server to stop once the run is over
fn handle_active(line: Option<&[u8]>, controller: &ainz::RunController, out: &Out) -> bool {
  let request = match line.map(parse) {
    Some(Ok(request)) => request,
    Some(Err(rejected)) => {
      out.send(&rejected);
      return false;
    }
    None => {
      out.send(&failure(
        Value::Null,
        PARSE_ERROR,
        "request line is too long",
      ));
      return false;
    }
  };
  match request.method.as_str() {
    "steer" => match request.params.get("message").and_then(Value::as_str) {
      Some(message) => match controller.try_steer(message) {
        Ok(()) => out.reply(request.id, json!({"queued": true})),
        Err(reason) => out.fail(request.id, CONTROL_CLOSED, reason),
      },
      None => out.fail(request.id, INVALID_PARAMS, "message is required"),
    },
    "cancel" => {
      if controller.cancel() {
        out.reply(request.id, json!({"cancelled": true}));
      } else {
        out.fail(request.id, CONTROL_CLOSED, "run control is closed");
      }
    }
    "shutdown" => {
      controller.cancel();
      out.reply(request.id, json!({"shutdown": true}));
      return true;
    }
    _ => out.fail(request.id, RUN_ACTIVE, "a run is already active"),
  }
  false
}

// bytes rather than text, so a line that is not UTF-8 is a parse error to answer rather than
// the end of the input
fn parse(line: &[u8]) -> Result<Request, Value> {
  let value: Value = serde_json::from_slice(line)
    .map_err(|error| failure(Value::Null, PARSE_ERROR, &format!("parse error: {error}")))?;
  let id = value.get("id").cloned();
  let Some(method) = value.get("method").and_then(Value::as_str) else {
    return Err(failure(
      id.unwrap_or(Value::Null),
      INVALID_REQUEST,
      "a request needs a method",
    ));
  };
  Ok(Request {
    id,
    method: method.to_string(),
    params: value.get("params").cloned().unwrap_or(Value::Null),
  })
}

fn failure(id: Value, code: i64, message: &str) -> Value {
  json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

// stdin read a line at a time into a buffer that outlives each call, so a read dropped by
// select! mid-line loses nothing and the next call carries on where it stopped
struct Input<R> {
  reader: BufReader<R>,
  buffer: Vec<u8>,
  skipping: bool,
}

impl<R: tokio::io::AsyncRead + Unpin> Input<R> {
  fn new(reader: R) -> Self {
    Self {
      reader: BufReader::new(reader),
      buffer: Vec::new(),
      skipping: false,
    }
  }

  // None at the end of input; Some(None) for a line too long to keep
  async fn next(&mut self) -> Result<Option<Option<Vec<u8>>>> {
    loop {
      if self.skipping {
        if !skip_line(&mut self.reader).await? {
          return Ok(None);
        }
        self.skipping = false;
        return Ok(Some(None));
      }
      let room = (MAX_LINE - self.buffer.len()) as u64;
      let read = (&mut self.reader)
        .take(room)
        .read_until(b'\n', &mut self.buffer)
        .await
        .context("read stdin")?;
      let ended = self.buffer.last() == Some(&b'\n');
      if !ended && read != 0 && self.buffer.len() >= MAX_LINE {
        self.buffer = Vec::new();
        self.skipping = true;
        continue;
      }
      if self.buffer.is_empty() {
        return Ok(None);
      }
      let line = std::mem::take(&mut self.buffer);
      if line.trim_ascii().is_empty() {
        continue;
      }
      return Ok(Some(Some(line)));
    }
  }
}

// throws away the rest of the current line without holding it; false at the end of input
async fn skip_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<bool> {
  loop {
    let available = reader.fill_buf().await.context("read stdin")?;
    if available.is_empty() {
      return Ok(false);
    }
    match available.iter().position(|byte| *byte == b'\n') {
      Some(end) => {
        reader.consume(end + 1);
        return Ok(true);
      }
      None => {
        let length = available.len();
        reader.consume(length);
      }
    }
  }
}

// every line leaving the process goes through one writer task, so an event written from
// inside a run can never land in the middle of a reply. None closes it: the event sink holds a
// sender too, so waiting for every sender to drop is no way to finish
#[derive(Clone)]
struct Out(mpsc::UnboundedSender<Option<String>>);

impl Out {
  fn send(&self, value: &Value) {
    drop(self.0.send(Some(value.to_string())));
  }

  fn reply(&self, id: Option<Value>, result: Value) {
    if let Some(id) = id {
      self.send(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }
  }

  fn fail(&self, id: Option<Value>, code: i64, message: &str) {
    if let Some(id) = id {
      self.send(&failure(id, code, message));
    }
  }

  fn close(&self) {
    drop(self.0.send(None));
  }
}

fn writer() -> (Out, JoinHandle<()>) {
  let (sender, mut queue) = mpsc::unbounded_channel::<Option<String>>();
  let task = tokio::spawn(async move {
    let mut stdout = BufWriter::new(tokio::io::stdout());
    while let Some(Some(line)) = queue.recv().await {
      if stdout.write_all(line.as_bytes()).await.is_err() || stdout.write_all(b"\n").await.is_err()
      {
        return;
      }
      // a burst of events goes out in one write, and nothing waits behind an idle buffer
      if queue.is_empty() && stdout.flush().await.is_err() {
        return;
      }
    }
    drop(stdout.flush().await);
  });
  (Out(sender), task)
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

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_request_without_an_id_is_a_notification_and_a_null_id_is_not() {
    assert_eq!(parse(br#"{"method": "cancel"}"#).unwrap().id, None);
    assert_eq!(
      parse(br#"{"id": null, "method": "state"}"#).unwrap().id,
      Some(Value::Null)
    );
  }

  #[test]
  fn a_line_that_is_not_utf8_is_a_parse_error_against_a_null_id() {
    let rejected = parse(b"{\"id\": 1, \"method\": \"\xff\"}").unwrap_err();
    assert_eq!(rejected["error"]["code"], json!(PARSE_ERROR));
    assert_eq!(rejected["id"], Value::Null);
  }

  #[test]
  fn a_request_with_no_method_is_rejected_against_its_own_id() {
    let rejected = parse(br#"{"id": 3, "params": {}}"#).unwrap_err();
    assert_eq!(rejected["error"]["code"], json!(INVALID_REQUEST));
    assert_eq!(rejected["id"], json!(3));
  }

  #[tokio::test]
  async fn input_survives_bad_bytes_and_drops_an_overlong_line_without_holding_it() {
    let mut text = b"\xff\xfe\n\n{\"method\":\"state\"}\n".to_vec();
    text.extend(std::iter::repeat_n(b'x', MAX_LINE + 10));
    text.extend(b"\n{\"id\":1,\"method\":\"state\"}");
    let mut input = Input::new(&text[..]);
    assert_eq!(
      input.next().await.unwrap(),
      Some(Some(b"\xff\xfe\n".to_vec()))
    );
    assert_eq!(
      input.next().await.unwrap(),
      Some(Some(b"{\"method\":\"state\"}\n".to_vec()))
    );
    assert_eq!(input.next().await.unwrap(), Some(None));
    assert_eq!(
      input.next().await.unwrap(),
      Some(Some(b"{\"id\":1,\"method\":\"state\"}".to_vec()))
    );
    assert_eq!(input.next().await.unwrap(), None);
  }
}
