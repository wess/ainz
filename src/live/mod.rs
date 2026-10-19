//! Live terminals: the host owns the PTY and worker; an attached terminal only relays bytes.

pub(crate) mod attention;
mod client;
mod host;
mod picker;
mod private;
mod terminal;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
  ffi::OsString,
  path::{Path, PathBuf},
  time::Duration,
};
use tokio::{
  io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
  net::UnixStream,
  sync::mpsc,
};
use uuid::Uuid;

pub(crate) use client::attach;
pub(crate) use host::serve;

const FRAME_LIMIT: usize = 128 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Info {
  pub id: Uuid,
  pub session: Option<Uuid>,
  pub workspace: PathBuf,
  pub title: String,
  #[serde(default)]
  pub label: Option<String>,
  #[serde(default)]
  pub unseen: bool,
  #[serde(default)]
  pub revision: u64,
  #[serde(default)]
  pub notifications: ainz::config::Notifications,
  #[serde(default)]
  pub delivery: ainz::config::NotificationDelivery,
  pub state: String,
  pub attached: bool,
  #[serde(default)]
  pub viewing: bool,
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub problem: Option<String>,
}

impl Info {
  pub fn attention(&self) -> Option<u8> {
    match self.state.as_str() {
      "needs approval" => Some(0),
      "failed run" if self.unseen => Some(1),
      "finished run" if self.unseen => Some(2),
      _ => None,
    }
  }
  pub fn badge(&self) -> &str {
    if self.unseen && self.state == "finished run" {
      "done · new"
    } else if self.unseen && self.state == "failed run" {
      "failed · new"
    } else if self.state == "finished run" {
      "ready"
    } else {
      &self.state
    }
  }
  pub fn short(&self) -> String {
    self.id.to_string()[28..].to_string()
  }
  pub fn live(&self) -> bool {
    !matches!(
      self.state.as_str(),
      "finished" | "failed" | "stopped" | "interrupted"
    )
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Action {
  Detach,
  Sessions,
  New,
  Next,
  Previous,
  Attention,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Message {
  Attach {
    cols: u16,
    rows: u16,
  },
  Worker,
  Inspect,
  Viewing(bool),
  Stop,
  Input(Vec<u8>),
  Output(Vec<u8>),
  Resize {
    cols: u16,
    rows: u16,
  },
  Redraw,
  State {
    session: Uuid,
    title: String,
    #[serde(default)]
    label: Option<String>,
    state: String,
    #[serde(default)]
    notifications: ainz::config::Notifications,
    #[serde(default)]
    delivery: ainz::config::NotificationDelivery,
  },
  Action(Action),
  Info(Info),
  Error(String),
  End,
}

pub(crate) async fn read<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<Message>> {
  let mut line = Vec::new();
  loop {
    let bytes = reader.fill_buf().await?;
    if bytes.is_empty() {
      if line.is_empty() {
        return Ok(None);
      }
      bail!("incomplete live-session message");
    }
    let count = bytes
      .iter()
      .position(|byte| *byte == b'\n')
      .map_or(bytes.len(), |n| n + 1);
    if line.len() + count > FRAME_LIMIT {
      bail!("live-session message is too large");
    }
    let done = bytes[count - 1] == b'\n';
    line.extend_from_slice(&bytes[..count]);
    reader.consume(count);
    if done {
      return Ok(Some(serde_json::from_slice(&line)?));
    }
  }
}

pub(crate) async fn write<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> Result<()> {
  let mut bytes = serde_json::to_vec(message)?;
  bytes.push(b'\n');
  writer.write_all(&bytes).await?;
  Ok(())
}

pub(crate) fn root() -> Result<PathBuf> {
  Ok(
    dirs::data_local_dir()
      .context("no user data directory")?
      .join("ainz/live"),
  )
}
fn socketdir() -> Result<PathBuf> {
  // macOS limits socket paths to 104 bytes; user data and home paths can exceed that.
  Ok(PathBuf::from(format!("/tmp/ainz-{}", unsafe {
    libc::geteuid()
  })))
}

pub(crate) fn socket(id: Uuid) -> Result<PathBuf> {
  Ok(socketdir()?.join(format!("{id}.sock")))
}
fn record(id: Uuid) -> Result<PathBuf> {
  Ok(root()?.join(format!("{id}.json")))
}

pub(crate) async fn connect(id: Uuid) -> Result<UnixStream> {
  UnixStream::connect(socket(id)?)
    .await
    .context("session is no longer running; use ainz resume for its saved conversation")
}

pub(crate) async fn inspect(id: Uuid) -> Result<Info> {
  let mut stream = connect(id).await?;
  write(&mut stream, &Message::Inspect).await?;
  match tokio::time::timeout(Duration::from_secs(2), read(&mut BufReader::new(stream))).await?? {
    Some(Message::Info(info)) => Ok(info),
    _ => bail!("session did not answer"),
  }
}

pub(crate) async fn list() -> Result<Vec<Info>> {
  let mut entries = match tokio::fs::read_dir(root()?).await {
    Ok(entries) => entries,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(error) => return Err(error.into()),
  };
  let mut infos = Vec::new();
  while let Some(entry) = entries.next_entry().await? {
    let path = entry.path();
    if path.extension().and_then(|s| s.to_str()) != Some("json") {
      continue;
    }
    let Some(id) = path
      .file_stem()
      .and_then(|s| s.to_str())
      .and_then(|s| Uuid::parse_str(s).ok())
    else {
      continue;
    };
    let Ok(bytes) = tokio::fs::read(&path).await else {
      continue;
    };
    let Ok(mut info) = serde_json::from_slice::<Info>(&bytes) else {
      continue;
    };
    if info.id != id {
      continue;
    }
    if info.live() {
      match inspect(id).await {
        Ok(current) => info = current,
        Err(_) => {
          info.state = "interrupted".into();
          info.attached = false;
          info.viewing = false;
        }
      }
    }
    infos.push(info);
  }
  infos.sort_by_key(|info| std::cmp::Reverse(info.id));
  Ok(infos)
}

pub(crate) async fn resolve(value: &str) -> Result<Info> {
  let matches: Vec<_> = list()
    .await?
    .into_iter()
    .filter(|info| {
      let id = info.id.to_string();
      info.live()
        && info
          .label
          .as_deref()
          .is_some_and(|name| name.eq_ignore_ascii_case(value))
        || id == value
        || (value.len() >= 4 && (id.starts_with(value) || id.ends_with(value)))
        || info.session.is_some_and(|id| id.to_string() == value)
    })
    .collect();
  match matches.as_slice() {
    [info] => Ok(info.clone()),
    [] => bail!("no live session matches {value}; ainz sessions lists them"),
    _ => bail!("{value} matches several sessions; use a unique name or longer id"),
  }
}

pub(crate) async fn stop(value: &str) -> Result<()> {
  let info = resolve(value).await?;
  if !info.live() {
    bail!("session {} is already {}", info.short(), info.state);
  }
  let mut stream = connect(info.id).await?;
  write(&mut stream, &Message::Stop).await?;
  match read(&mut BufReader::new(stream)).await? {
    Some(Message::Info(_)) => {
      println!("stopping session {}", info.short());
      Ok(())
    }
    Some(Message::Error(error)) => bail!("{error}"),
    _ => bail!("session did not acknowledge stop"),
  }
}

#[derive(Serialize, Deserialize)]
struct Launch {
  cwd: PathBuf,
  args: Vec<OsString>,
  workspace: PathBuf,
  cols: u16,
  rows: u16,
}

pub(crate) async fn start(workspace: &Path, args: Vec<OsString>) -> Result<Uuid> {
  use std::os::unix::process::CommandExt;
  private::dir(&root()?).await?;
  private::dir(&socketdir()?).await?;
  let id = Uuid::now_v7();
  let (cols, rows) = ratatui::crossterm::terminal::size().unwrap_or((100, 30));
  let launch = Launch {
    cwd: if args.is_empty() {
      workspace.to_path_buf()
    } else {
      std::env::current_dir().unwrap_or_else(|_| workspace.to_path_buf())
    },
    args,
    workspace: workspace.to_path_buf(),
    cols,
    rows,
  };
  let path = root()?.join(format!("{id}.launch"));
  private::write(&path, serde_json::to_vec(&launch)?).await?;
  let mut command = std::process::Command::new(std::env::current_exe()?);
  command
    .arg("--workspace")
    .arg(workspace)
    .arg("host")
    .arg(id.to_string());
  command
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());
  // Only async-signal-safe calls may run between fork and exec in this multithreaded process.
  unsafe {
    command.pre_exec(|| {
      if libc::setsid() < 0 {
        return Err(std::io::Error::last_os_error());
      }
      Ok(())
    });
  }
  let mut child = command.spawn().context("start session host")?;
  let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
  loop {
    if inspect(id).await.is_ok() {
      return Ok(id);
    }
    if let Some(status) = child.try_wait()? {
      drop(tokio::fs::remove_file(&path).await);
      use std::io::Read;
      let mut details = String::new();
      if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(8192).read_to_string(&mut details);
      }
      bail!(
        "session host could not start ({status}): {}",
        details.trim()
      );
    }
    if tokio::time::Instant::now() >= deadline {
      drop(child.kill());
      drop(child.wait());
      drop(tokio::fs::remove_file(&path).await);
      bail!("session host did not start within 10 seconds");
    }
    tokio::time::sleep(Duration::from_millis(30)).await;
  }
}

pub(crate) enum Control {
  Redraw,
  Stop,
}

pub(crate) struct Link {
  pub id: Uuid,
  send: mpsc::UnboundedSender<Message>,
  pub redraw: mpsc::UnboundedReceiver<Control>,
  last: Option<String>,
}

impl Link {
  pub(crate) async fn open(id: Uuid) -> Result<Self> {
    let mut socket = connect(id).await?;
    write(&mut socket, &Message::Worker).await?;
    let (readhalf, mut writehalf) = socket.into_split();
    let (send, mut queue) = mpsc::unbounded_channel();
    let (redraw, receive) = mpsc::unbounded_channel();
    tokio::spawn(async move {
      let mut reader = BufReader::new(readhalf);
      tokio::select! {
        _ = async {
          while let Ok(Some(message)) = read(&mut reader).await {
            let request = match message { Message::Redraw => Control::Redraw, Message::Stop => Control::Stop, _ => break };
            let _ = redraw.send(request);
          }
        } => {},
        _ = async {
          while let Some(message) = queue.recv().await {
            if write(&mut writehalf, &message).await.is_err() { break; }
          }
        } => {},
      }
    });
    Ok(Self {
      id,
      send,
      redraw: receive,
      last: None,
    })
  }

  pub(crate) fn state(
    &mut self,
    session: Uuid,
    title: String,
    label: Option<String>,
    state: &str,
    ui: &ainz::config::UiConfig,
  ) {
    let message = Message::State {
      session,
      title,
      label,
      state: state.into(),
      notifications: ui.notifications,
      delivery: ui.notification_delivery,
    };
    let next = serde_json::to_string(&message).expect("live state is serializable");
    if self.last.as_ref() != Some(&next) {
      drop(self.send.send(message));
      self.last = Some(next);
    }
  }
  pub(crate) fn action(&self, action: Action) {
    drop(self.send.send(Message::Action(action)));
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[tokio::test]
  async fn a_message_split_across_reads_is_followed_by_the_next_whole_message() {
    use tokio::io::AsyncWriteExt;
    let (mut send, receive) = tokio::io::duplex(16);
    let writer = tokio::spawn(async move {
      send.write_all(b"\"Ins").await.unwrap();
      tokio::task::yield_now().await;
      send.write_all(b"pect\"\n\"Stop\"\n").await.unwrap();
    });
    let mut reader = BufReader::new(receive);
    assert!(matches!(
      read(&mut reader).await.unwrap(),
      Some(Message::Inspect)
    ));
    assert!(matches!(
      read(&mut reader).await.unwrap(),
      Some(Message::Stop)
    ));
    assert!(read(&mut reader).await.unwrap().is_none());
    writer.await.unwrap();
  }

  #[tokio::test]
  async fn a_peer_cannot_allocate_an_unbounded_message_or_leave_a_partial_one() {
    let bytes = vec![b'x'; FRAME_LIMIT + 1];
    let mut too_large = BufReader::new(&bytes[..]);
    assert!(
      read(&mut too_large)
        .await
        .unwrap_err()
        .to_string()
        .contains("too large")
    );
    let mut incomplete = BufReader::new(&b"\"Inspect\""[..]);
    assert!(
      read(&mut incomplete)
        .await
        .unwrap_err()
        .to_string()
        .contains("incomplete")
    );
  }
}
