use super::{Action, Info, Launch, Message, read, record, root, socket, terminal, write};
use anyhow::{Result, bail};
use std::time::Duration;
use tokio::{
  io::BufReader,
  net::{UnixListener, UnixStream},
  sync::mpsc,
};
use uuid::Uuid;

enum Arrived {
  Open(Uuid, Message, mpsc::Sender<Message>),
  Message(Uuid, Message),
  Closed(Uuid),
}

async fn peer(stream: UnixStream, events: mpsc::Sender<Arrived>) -> Result<()> {
  if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
    bail!("session belongs to another user");
  }
  let id = Uuid::now_v7();
  let (readhalf, mut writehalf) = stream.into_split();
  let mut reader = BufReader::new(readhalf);
  let Some(first) = tokio::time::timeout(Duration::from_secs(3), read(&mut reader)).await?? else {
    return Ok(());
  };
  let (send, mut queue) = mpsc::channel(32);
  events.send(Arrived::Open(id, first, send)).await?;
  tokio::select! {
    result = async {
      while let Some(message) = read(&mut reader).await? {
        events.send(Arrived::Message(id, message)).await?;
      }
      Ok::<_, anyhow::Error>(())
    } => { drop(result); },
    result = async {
      while let Some(message) = queue.recv().await {
        tokio::time::timeout(Duration::from_secs(3), write(&mut writehalf, &message)).await??;
        if matches!(message, Message::End) { break; }
      }
      Ok::<_, anyhow::Error>(())
    } => { drop(result); },
  }
  drop(events.send(Arrived::Closed(id)).await);
  Ok(())
}

async fn save(info: &Info) -> Result<()> {
  super::private::write(&record(info.id)?, serde_json::to_vec(info)?).await
}

fn send(peer: &mut Option<(Uuid, mpsc::Sender<Message>)>, message: Message) {
  if peer
    .as_ref()
    .is_some_and(|(_, send)| send.try_send(message).is_err())
  {
    *peer = None;
  }
}

pub(crate) async fn serve(id: Uuid) -> Result<()> {
  let launchpath = root()?.join(format!("{id}.launch"));
  let launch: Launch = serde_json::from_slice(&tokio::fs::read(&launchpath).await?)?;
  tokio::fs::remove_file(&launchpath).await?;
  serve_launch(id, launch).await
}

async fn serve_launch(id: Uuid, launch: Launch) -> Result<()> {
  use std::os::unix::fs::PermissionsExt;
  super::private::dir(&root()?).await?;
  super::private::dir(&super::socketdir()?).await?;
  let socketpath = socket(id)?;
  let listener = UnixListener::bind(&socketpath)?;
  tokio::fs::set_permissions(&socketpath, std::fs::Permissions::from_mode(0o600)).await?;
  let mut info = Info {
    id,
    session: None,
    workspace: launch.workspace.clone(),
    title: "new session".into(),
    label: None,
    unseen: false,
    revision: 0,
    notifications: ainz::config::Notifications::Off,
    delivery: ainz::config::NotificationDelivery::Terminal,
    state: "starting".into(),
    attached: false,
    viewing: false,
    problem: None,
  };
  save(&info).await?;
  let mut command = tokio::process::Command::new(std::env::current_exe()?);
  command
    .kill_on_drop(true)
    .arg("--live-worker")
    .arg(id.to_string())
    .current_dir(&launch.cwd);
  if launch.args.is_empty() {
    command.arg("--workspace").arg(&launch.workspace);
  }
  command.args(&launch.args);
  let (pty, mut child) = match terminal::spawn(&mut command, launch.cols, launch.rows) {
    Ok(pair) => pair,
    Err(error) => {
      info.state = "failed".into();
      info.problem = Some(format!("{error:#}"));
      save(&info).await?;
      drop(tokio::fs::remove_file(&socketpath).await);
      return Err(error);
    }
  };
  let pid = child.id().expect("spawned worker has a pid");
  let (events, mut incoming) = mpsc::channel(64);
  let mut peers = tokio::task::JoinSet::new();
  let mut attached: Option<(Uuid, mpsc::Sender<Message>)> = None;
  let mut worker: Option<(Uuid, mpsc::Sender<Message>)> = None;
  let mut notification: Option<tokio::task::JoinHandle<()>> = None;
  let mut stopping = false;
  let mut deadline = None;
  let mut terminal_open = true;
  let mut tail = Vec::new();
  use tokio::signal::unix::{SignalKind, signal};
  let mut terminate = signal(SignalKind::terminate())?;
  let mut hangup = signal(SignalKind::hangup())?;
  let result = loop {
    tokio::select! {
      status = child.wait() => {
        info.state = if stopping { "stopped" } else if status.as_ref().is_ok_and(|s| s.success()) { "finished" } else { "failed" }.into();
        if info.state == "failed" {
          while let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_millis(30), pty.read()).await {
            if bytes.is_empty() { break; }
            keep_tail(&mut tail, &bytes);
          }
          let text = String::from_utf8_lossy(&tail);
          info.problem = Some(text.rfind("error:").or_else(|| text.rfind("panicked at"))
            .map(|at| text[at..].lines().next().unwrap_or_default().chars().filter(|c| !c.is_control()).take(500).collect())
            .unwrap_or_else(|| format!("worker exited: {status:?}")));
        }
        break status.map(|_| ()).map_err(anyhow::Error::from);
      },
      accepted = listener.accept() => {
        if let Ok((socket, _)) = accepted {
          let events = events.clone();
          peers.spawn(async move { drop(peer(socket, events).await); });
        }
      },
      Some(_) = peers.join_next(), if !peers.is_empty() => {},
      bytes = pty.read(), if terminal_open => {
        match bytes {
          Ok(bytes) if !bytes.is_empty() => {
            keep_tail(&mut tail, &bytes);
            send(&mut attached, Message::Output(bytes));
          },
          _ => terminal_open = false,
        }
      },
      Some(event) = incoming.recv() => match event {
        Arrived::Open(token, request, reply) => match request {
          Message::Inspect => { drop(reply.try_send(Message::Info(info.clone()))); },
          Message::Stop => {
            stopping = true;
            deadline = Some(tokio::time::Instant::now() + Duration::from_secs(10));
            send(&mut worker, Message::Stop);
            drop(reply.try_send(Message::Info(info.clone())));
          },
          Message::Attach { cols, rows } if attached.is_none() && !stopping => {
            drop(pty.resize(cols, rows));
            info.attached = true;
            info.viewing = true;
            info.unseen = false;
            save(&info).await?;
            drop(reply.try_send(Message::Info(info.clone())));
            attached = Some((token, reply));
            send(&mut worker, Message::Redraw);
          },
          Message::Attach { .. } => { drop(reply.try_send(Message::Error("session is already attached in another terminal".into()))); },
          Message::Worker if worker.is_none() => {
            worker = Some((token, reply));
            send(&mut worker, Message::Redraw);
          },
          _ => { drop(reply.try_send(Message::Error("invalid session handshake".into()))); },
        },
        Arrived::Message(token, message) if attached.as_ref().is_some_and(|(id, _)| *id == token) => match message {
          Message::Viewing(viewing) => {
            info.viewing = viewing;
            if viewing { info.unseen = false; }
            save(&info).await?;
          },
          Message::Input(bytes) if bytes.len() <= 8192 => { drop(pty.write(&bytes).await); },
          Message::Resize { cols, rows } => { drop(pty.resize(cols, rows)); send(&mut worker, Message::Redraw); },
          Message::Redraw => send(&mut worker, Message::Redraw),
          _ => {},
        },
        Arrived::Message(token, message) if worker.as_ref().is_some_and(|(id, _)| *id == token) => match message {
          message @ Message::State { .. } => {
            let changed = super::attention::apply(&mut info, &message);
            save(&info).await?;
            if changed && !info.viewing && info.notifications.accepts(&info.state) &&
              info.delivery == ainz::config::NotificationDelivery::System {
              if let Some(previous) = notification.take() { previous.abort(); }
              let info = info.clone();
              notification = Some(tokio::spawn(async move { super::attention::system(&info).await; }));
            }
          },
          Message::Action(action) => {
            send(&mut attached, Message::Action(action.clone()));
            if matches!(action, Action::Detach) { attached = None; }
          },
          _ => {},
        },
        Arrived::Closed(token) => {
          if attached.as_ref().is_some_and(|(id, _)| *id == token) { attached = None; }
          if worker.as_ref().is_some_and(|(id, _)| *id == token) { worker = None; }
        },
        _ => {},
      },
      _ = terminate.recv() => {
        stopping = true;
        deadline = Some(tokio::time::Instant::now() + Duration::from_secs(10));
        send(&mut worker, Message::Stop);
      },
      // A host has no terminal of its own; a launching shell's hangup must not kill its worker.
      _ = hangup.recv() => {},
      _ = async { tokio::time::sleep_until(deadline.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400))).await }, if deadline.is_some() => {
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
        deadline = None;
      },
    }
    let connected = attached.is_some();
    if info.attached != connected {
      info.attached = connected;
      if !connected {
        info.viewing = false;
      }
      save(&info).await?;
    }
  };
  // Nothing detached depends on a reader or writer owned by an attached client.
  info.attached = false;
  info.viewing = false;
  save(&info).await?;
  send(&mut attached, Message::Info(info.clone()));
  send(&mut attached, Message::End);
  drop(attached);
  drop(worker);
  let _ = tokio::time::timeout(Duration::from_millis(500), async {
    while peers.join_next().await.is_some() {}
  })
  .await;
  peers.abort_all();
  drop(tokio::fs::remove_file(&socketpath).await);
  result
}

fn keep_tail(tail: &mut Vec<u8>, bytes: &[u8]) {
  tail.extend_from_slice(bytes);
  if tail.len() > 8192 {
    tail.drain(..tail.len() - 8192);
  }
}
