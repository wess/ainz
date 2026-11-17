//! A headless coding CLI as a provider. The command runs its own loop and answers once, so
//! Ainz's tools stand aside. The run's mode is passed through in whatever flag the command
//! understands, and a command that can send its permission requests back has them answered by
//! the run's rules and the person, the same as a call Ainz makes itself.

use std::{path::PathBuf, process::Stdio};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use tokio::{
  io::{AsyncWriteExt, BufReader},
  process::Command,
  sync::mpsc,
};

use super::{
  ChatProvider, MAX_TRANSFER, ProviderReply, bridge, input,
  stream::{Completion, StreamState, next_line},
};
use crate::{
  agent::{Approver, deny_all},
  config::{PermissionMode, ProcessOutput},
  event::{Event, EventSink},
  process::GroupGuard,
  protocol::{Message, Role, ToolSpec},
};

#[derive(Clone)]
pub struct ProcessProvider {
  command: String,
  args: Vec<String>,
  model: String,
  workspace: PathBuf,
  /// The mode for a request made outside a run, which brings none of its own.
  permissions: PermissionMode,
  output: ProcessOutput,
  /// Where a picture whose file is gone is written out again for the command to open.
  images: PathBuf,
}

impl ProcessProvider {
  pub fn new(
    command: String,
    args: Vec<String>,
    model: String,
    workspace: PathBuf,
    permissions: PermissionMode,
    output: ProcessOutput,
  ) -> Self {
    Self {
      command,
      args,
      model,
      workspace,
      permissions,
      output,
      images: input::images(),
    }
  }

  /// Writes pictures the command is handed somewhere other than the cache.
  pub fn with_images(mut self, folder: PathBuf) -> Self {
    self.images = folder;
    self
  }

  pub(crate) fn with_model(&self, model: String) -> Self {
    Self {
      model,
      ..self.clone()
    }
  }

  fn bridged(&self) -> bool {
    self.output == ProcessOutput::StreamJson
      && input::reads_json(&self.args)
      && bridge::bridged(&self.args)
  }

  fn args(&self, mode: PermissionMode) -> Vec<String> {
    let workspace = self.workspace.to_string_lossy();
    let sandbox = match mode {
      PermissionMode::Auto => "workspace-write",
      PermissionMode::Ask | PermissionMode::ReadOnly => "read-only",
    };
    let permission = match mode {
      PermissionMode::Auto => "acceptEdits",
      // a command that can ask runs in its own default mode and asks; one that cannot would be
      // refused everything it asks about, so it plans instead
      PermissionMode::Ask if self.bridged() => "default",
      PermissionMode::Ask | PermissionMode::ReadOnly => "plan",
    };
    self
      .args
      .iter()
      .map(|arg| {
        arg
          .replace("{model}", &self.model)
          .replace("{workspace}", &workspace)
          .replace("{sandbox}", sandbox)
          .replace("{permission}", permission)
      })
      .collect()
  }
}

#[async_trait]
impl ChatProvider for ProcessProvider {
  async fn complete(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    events: &EventSink,
  ) -> Result<ProviderReply> {
    // outside a run there is nobody to ask
    self
      .run(messages, events, self.permissions, &deny_all())
      .await
  }

  async fn complete_asking(
    &self,
    messages: &[&Message],
    _tools: &[ToolSpec],
    events: &EventSink,
    mode: PermissionMode,
    asks: &Approver,
  ) -> Result<ProviderReply> {
    self.run(messages, events, mode, asks).await
  }

  fn runs_tools(&self) -> bool {
    true
  }
}

impl ProcessProvider {
  async fn run(
    &self,
    messages: &[&Message],
    events: &EventSink,
    mode: PermissionMode,
    asks: &Approver,
  ) -> Result<ProviderReply> {
    // built first so a turn the command cannot take fails before anything is started
    let prompt = input::render(messages, input::reads_json(&self.args), &self.images).await?;
    let bridged = self.bridged();
    let mut child = Command::new(&self.command)
      .args(self.args(mode))
      .current_dir(&self.workspace)
      .stdin(Stdio::piped())
      .stdout(Stdio::piped())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .process_group(0)
      .spawn()
      .with_context(|| format!("start provider command {}", self.command))?;
    let _guard = GroupGuard::new(child.id());
    let mut stdin = child
      .stdin
      .take()
      .context("provider command stdin was unavailable")?;
    let stdout = child
      .stdout
      .take()
      .context("provider command stdout was unavailable")?;
    let stderr = child
      .stderr
      .take()
      .context("provider command stderr was unavailable")?;
    // the prompt is fed while output drains, so a chatty command cannot fill its pipe and
    // stall. a write error is left to the exit status: a command that quit early explains
    // itself on stderr. a bridged command keeps its stdin open for answers until it reports
    // its result
    let (writer, mut lines) = mpsc::unbounded_channel::<String>();
    drop(writer.send(prompt));
    let writer = bridged.then_some(writer);
    let feed = async move {
      while let Some(line) = lines.recv().await {
        if stdin.write_all(line.as_bytes()).await.is_err() {
          break;
        }
      }
      drop(stdin.shutdown().await);
    };
    // stderr drains on its own task for the same reason the prompt is fed on one
    let errors = tokio::spawn(async move { crate::output::capture(stderr, MAX_TRANSFER).await });
    let mut stream = StreamState::default();
    let read = async {
      // owned here, so stdin closes however reading ends, an error included
      let mut writer = writer;
      let mut reader = BufReader::new(stdout);
      let mut buffered = String::new();
      if self.output == ProcessOutput::StreamJson {
        // a line is acted on and dropped, so only its length costs memory: a limit on the whole
        // report would be a limit on how long the command may work, and every screenshot it
        // reads is a megabyte of that report
        while let Some(line) = next_line(&mut reader, MAX_TRANSFER)
          .await
          .context("read provider output")?
        {
          let request = stream.push(&String::from_utf8_lossy(&line), events);
          // the command waits on the answer, so nothing else is read until it is given
          if let (Some(request), Some(sender)) = (request, &writer)
            && let Some(answer) = bridge::answer(&request, asks).await
          {
            drop(sender.send(answer));
          }
          if stream.done {
            writer = None;
          }
        }
      } else {
        // the other modes want the whole of stdout, so read it in one piece
        let captured = crate::output::capture(reader, MAX_TRANSFER)
          .await
          .context("read provider output")?;
        if captured.truncated {
          bail!("provider output exceeds the transfer limit");
        }
        buffered = String::from_utf8(captured.bytes).context("provider output was not UTF-8")?;
      }
      anyhow::Ok(buffered)
    };
    let (_, buffered) = tokio::join!(feed, read);
    let buffered = buffered?;
    let status = child.wait().await.context("wait for provider command")?;
    let errors = errors.await.context("read provider stderr")??;
    let errors = String::from_utf8_lossy(&errors.bytes);
    if !status.success() {
      // a command that dies early may explain itself on either pipe
      let detail = match errors.trim() {
        "" => buffered.trim(),
        error => error,
      };
      bail!("provider command failed ({status}): {detail}");
    }

    let reply = match self.output {
      ProcessOutput::Text => Completion::whole(buffered.trim().to_string()),
      ProcessOutput::JsonResult => Completion::whole(
        serde_json::from_str::<serde_json::Value>(buffered.trim())
          .context("invalid provider JSON output")?
          .get("result")
          .and_then(serde_json::Value::as_str)
          .context("provider JSON output had no result string")?
          .to_string(),
      ),
      ProcessOutput::StreamJson => stream.finish()?,
    };
    // text that already reached the screen as it arrived must not be sent a second time
    if !reply.text.is_empty() && !reply.streamed {
      events.emit(Event::TextDelta {
        text: reply.text.clone(),
      });
    }
    Ok(ProviderReply {
      message: Message::text(Role::Assistant, reply.text),
      usage: reply.usage,
      truncated: false,
    })
  }
}
