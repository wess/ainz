use super::{Action, Info, Message, connect, list, read, resolve, start, write};
use anyhow::{Context, Result, bail};
use std::{
  io::{Read, Write},
  path::Path,
  sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
  },
  time::Duration,
};
use tokio::{io::BufReader, net::unix::OwnedWriteHalf, sync::mpsc};
use uuid::Uuid;

struct Terminal;
impl Terminal {
  fn enter() -> Result<Self> {
    ratatui::crossterm::terminal::enable_raw_mode()?;
    let guard = Self;
    print!("\x1b[?1049h\x1b[2J\x1b[H");
    std::io::stdout().flush()?;
    Ok(guard)
  }
}
impl Drop for Terminal {
  fn drop(&mut self) {
    drop(std::io::stdout().write_all(
      b"\x1b[=0u\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?25h\x1b[0m\x1b[?1049l",
    ));
    drop(std::io::stdout().flush());
    drop(ratatui::crossterm::terminal::disable_raw_mode());
  }
}

struct Connection {
  writer: OwnedWriteHalf,
  events: mpsc::Receiver<Message>,
  task: tokio::task::JoinHandle<()>,
  paused: Arc<AtomicBool>,
}
impl Drop for Connection {
  fn drop(&mut self) {
    self.task.abort();
  }
}

async fn open(id: Uuid) -> Result<Connection> {
  let mut socket = connect(id).await?;
  let (cols, rows) = ratatui::crossterm::terminal::size().unwrap_or((100, 30));
  write(&mut socket, &Message::Attach { cols, rows }).await?;
  let (readhalf, writer) = socket.into_split();
  let mut reader = BufReader::new(readhalf);
  match tokio::time::timeout(Duration::from_secs(3), read(&mut reader)).await?? {
    Some(Message::Info(_)) => {}
    Some(Message::Error(error)) => bail!("{error}"),
    _ => bail!("session did not accept the attachment"),
  }
  let (send, events) = mpsc::channel(64);
  let paused = Arc::new(AtomicBool::new(false));
  let pause = paused.clone();
  let task = tokio::spawn(async move {
    while let Ok(Some(message)) = read(&mut reader).await {
      if pause.load(Ordering::Relaxed) && matches!(message, Message::Output(_)) {
        continue;
      }
      if send.send(message).await.is_err() {
        break;
      }
    }
  });
  Ok(Connection {
    writer,
    events,
    task,
    paused,
  })
}

fn clipped(text: &str, width: u16) -> String {
  let mut used = 0;
  text
    .chars()
    .filter(|c| !c.is_control())
    .take_while(|c| {
      used += ratatui::text::Span::raw(c.to_string()).width();
      used <= usize::from(width)
    })
    .collect()
}

fn draw_picker(
  infos: &[Info],
  index: usize,
  problem: Option<&str>,
  query: &str,
  searching: bool,
) -> Result<()> {
  let (cols, rows) = ratatui::crossterm::terminal::size().unwrap_or((100, 30));
  let shown = (usize::from(rows.saturating_sub(9)) / 2)
    .max(1)
    .min(infos.len());
  let first = index
    .saturating_sub(shown / 2)
    .min(infos.len().saturating_sub(shown));
  print!(
    "\x1b[=0u\x1b[?1000l\x1b[?1002l\x1b[?1006l\x1b[?25l\x1b[?2004h\x1b[0m\x1b[2J\x1b[H Ainz sessions\r\n\r\n"
  );
  let mut group = None;
  for (at, info) in infos.iter().enumerate().skip(first).take(shown) {
    if group != Some(&info.workspace) {
      let heading = clipped(&info.workspace.to_string_lossy(), cols.saturating_sub(3));
      print!(" {heading}\r\n");
      group = Some(&info.workspace);
    }
    let row = format!(
      " {} {}  {:<14} {}{}",
      if at == index { "›" } else { " " },
      info.short(),
      info.badge(),
      info.title,
      if info.attached { " · attached" } else { "" }
    );
    let row = clipped(&row, cols.saturating_sub(1));
    if at == index {
      print!("\x1b[48;2;24;66;128m\x1b[38;2;240;238;250m{row}\x1b[0m\r\n");
    } else {
      print!("{row}\r\n");
    }
  }
  if infos.is_empty() {
    print!(" No matching sessions. Press n to start one, or / to change the search.\r\n");
  }
  let prompt = clipped(query, cols.saturating_sub(12));
  print!(
    "\r\n / search: {prompt}{}\r\n",
    if searching { "▏" } else { "" }
  );
  print!(" ↑/↓ select · enter attach · / search · n new · r refresh · esc back\r\n");
  if let Some(problem) = problem {
    print!(" {}\r\n", clipped(problem, cols.saturating_sub(2)));
  }
  std::io::stdout().flush()?;
  Ok(())
}

fn filtered(infos: &[Info], query: &str) -> Vec<Info> {
  let mut rows: Vec<_> = infos
    .iter()
    .filter(|info| info.live())
    .filter(|info| {
      let needle = query.to_lowercase();
      info.title.to_lowercase().contains(&query.to_lowercase())
        || info
          .workspace
          .to_string_lossy()
          .to_lowercase()
          .contains(&needle)
        || info.short().contains(&needle)
        || info.state.to_lowercase().contains(&needle)
    })
    .cloned()
    .collect();
  rows.sort_by(|a, b| a.workspace.cmp(&b.workspace).then(a.id.cmp(&b.id)));
  rows
}

async fn picker(
  input: &mut mpsc::Receiver<Vec<u8>>,
  current: Option<Uuid>,
  workspace: &Path,
  monitor: &mut super::attention::Monitor,
  seen: &mut std::collections::HashMap<Uuid, u64>,
) -> Result<Option<Uuid>> {
  let mut all = list().await?;
  let mut query = String::new();
  let mut searching = false;
  let mut infos = filtered(&all, &query);
  let mut index = infos
    .iter()
    .position(|info| Some(info.id) == current)
    .unwrap_or(0);
  let mut problem = None;
  let mut parser = super::picker::Input::default();
  use super::picker::{Key, parse};
  loop {
    draw_picker(&infos, index, problem.as_deref(), &query, searching)?;
    let bytes = tokio::select! {
      bytes = input.recv() => match bytes { Some(bytes) => bytes, None => return Ok(None) },
      _ = tokio::time::sleep(Duration::from_millis(50)), if parser.escape == [27] => {
        parser.escape.clear();
        if searching { searching = false; query.clear(); infos = filtered(&all, &query); index = 0; }
        else { return Ok(current); }
        continue;
      },
      Some(updated) = monitor.updates.recv() => {
        super::attention::terminal(&updated, seen, current.unwrap_or(Uuid::nil()))?;
        let selected = infos.get(index).map(|info| info.id);
        all = updated;
        infos = filtered(&all, &query);
        index = infos.iter().position(|info| Some(info.id) == selected).unwrap_or(0);
        continue;
      },
    };
    for byte in bytes {
      let Some(key) = parse(&mut parser, byte) else {
        continue;
      };
      match key {
        Key::Cancel => return Ok(None),
        Key::Up => index = index.saturating_sub(1),
        Key::Down => index = (index + 1).min(infos.len().saturating_sub(1)),
        Key::Enter => {
          if let Some(info) = infos.get(index) {
            if info.attached && Some(info.id) != current {
              problem =
                Some("Already attached in another terminal. Detach it there first.".to_string());
            } else {
              return Ok(Some(info.id));
            }
          }
        }
        Key::Clear => {
          query.clear();
          index = 0;
        }
        Key::Backspace if searching => {
          query.pop();
          index = 0;
        }
        Key::Char('/') if !searching => searching = true,
        Key::Char('k') if !searching => index = index.saturating_sub(1),
        Key::Char('j') if !searching => index = (index + 1).min(infos.len().saturating_sub(1)),
        Key::Char('n') if !searching => return Ok(Some(start(workspace, Vec::new()).await?)),
        Key::Char('r') if !searching => {
          all = list().await?;
          problem = None;
        }
        Key::Text(c) => {
          searching = true;
          if query.chars().count() < 120 {
            query.push(c);
          }
          index = 0;
        }
        Key::Char(c) if searching => {
          if query.chars().count() < 120 {
            query.push(c);
          }
          index = 0;
        }
        _ => {}
      }
      infos = filtered(&all, &query);
      index = index.min(infos.len().saturating_sub(1));
    }
  }
}

pub(crate) async fn attach(
  value: Option<&str>,
  initial: Option<Uuid>,
  workspace: &Path,
) -> Result<()> {
  use std::io::IsTerminal;
  if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
    bail!("attach needs an interactive terminal");
  }
  let first = match (value, initial) {
    (_, Some(id)) => Some(id),
    (Some(value), _) => {
      let info = resolve(value).await?;
      if !info.live() {
        bail!(
          "session {} is {}: {}{}",
          info.short(),
          info.badge(),
          info
            .problem
            .as_deref()
            .unwrap_or("the live process has ended"),
          info
            .session
            .map(|id| format!("; continue with ainz resume {id}"))
            .unwrap_or_default()
        );
      }
      Some(info.id)
    }
    _ => {
      let infos: Vec<_> = list()
        .await?
        .into_iter()
        .filter(|info| info.live() && !info.attached)
        .collect();
      if infos.len() == 1 {
        Some(infos[0].id)
      } else {
        None
      }
    }
  };
  let mut monitor = super::attention::monitor().await?;
  let mut seen = std::collections::HashMap::new();
  let terminal = Terminal::enter()?;
  let (keys, mut input) = mpsc::channel(32);
  std::thread::spawn(move || {
    let mut stdin = std::io::stdin().lock();
    let mut bytes = [0; 4096];
    loop {
      match stdin.read(&mut bytes) {
        Ok(0) | Err(_) => break,
        Ok(count) => {
          if keys.blocking_send(bytes[..count].to_vec()).is_err() {
            break;
          }
        }
      }
    }
  });
  let Some(mut id) = (match first {
    Some(id) => Some(id),
    None => picker(&mut input, None, workspace, &mut monitor, &mut seen).await?,
  }) else {
    return Ok(());
  };
  let mut connection = open(id).await?;
  use tokio::signal::unix::{SignalKind, signal};
  let mut resize = signal(SignalKind::window_change())?;
  let mut hangup = signal(SignalKind::hangup())?;
  let mut terminate = signal(SignalKind::terminate())?;
  let mut detached = false;
  let mut ended = None;
  let mut lost = false;
  loop {
    let mut choose = false;
    let mut next = None;
    tokio::select! {
      Some(infos) = monitor.updates.recv() => { super::attention::terminal(&infos, &mut seen, id)?; },
      bytes = input.recv() => {
        let Some(bytes) = bytes else { detached = true; break; };
        write(&mut connection.writer, &Message::Input(bytes)).await?;
      },
      message = connection.events.recv() => match message {
        Some(Message::Output(bytes)) => { std::io::stdout().write_all(&bytes)?; std::io::stdout().flush()?; },
        Some(Message::Action(Action::Detach)) => { detached = true; },
        Some(Message::Action(Action::Sessions)) => choose = true,
        Some(Message::Action(Action::New)) => {
          let current = super::inspect(id).await?;
          next = Some(start(&current.workspace, Vec::new()).await?);
        },
        Some(Message::Action(action @ (Action::Next | Action::Previous))) => {
          let infos: Vec<_> = list().await?.into_iter().filter(|info| info.live() && (!info.attached || info.id == id)).collect();
          if let Some(at) = infos.iter().position(|info| info.id == id) {
            let delta = if matches!(action, Action::Next) { 1 } else { infos.len().saturating_sub(1) };
            next = Some(infos[(at + delta) % infos.len()].id);
          }
        },
        Some(Message::Action(Action::Attention)) => {
          next = super::attention::next(&list().await?, id);
          if next.is_none() {
            draw_picker(&[], 0, Some("No other session needs attention."), "", false)?;
            tokio::time::sleep(Duration::from_millis(700)).await;
            next = Some(id);
          }
        },
        Some(Message::End) => break,
        None => { lost = true; break; },
        Some(Message::Info(info)) => ended = Some(info),
        Some(Message::Error(error)) => bail!("{error}"),
        _ => {},
      },
      _ = resize.recv() => {
        let (cols, rows) = ratatui::crossterm::terminal::size()?;
        write(&mut connection.writer, &Message::Resize { cols, rows }).await?;
      },
      _ = hangup.recv() => { detached = true; },
      _ = terminate.recv() => { detached = true; },
    }
    if detached {
      break;
    }
    if choose {
      write(&mut connection.writer, &Message::Viewing(false)).await?;
      connection.paused.store(true, Ordering::Relaxed);
      while connection.events.try_recv().is_ok() {}
      let current = super::inspect(id).await?;
      next = picker(
        &mut input,
        Some(id),
        &current.workspace,
        &mut monitor,
        &mut seen,
      )
      .await?;
      connection.paused.store(false, Ordering::Relaxed);
      if next.is_none() {
        detached = true;
        break;
      }
    }
    if let Some(target) = next {
      if target != id {
        // Attach the destination before releasing the current session, so a race with another
        // terminal leaves this one on its original session rather than stranded.
        match open(target).await {
          Ok(new) => {
            connection = new;
            id = target;
          }
          Err(error) => {
            draw_picker(&[], 0, Some(&format!("{error:#}")), "", false)?;
            tokio::time::sleep(Duration::from_millis(900)).await;
          }
        }
      }
      while connection.events.try_recv().is_ok() {}
      write(&mut connection.writer, &Message::Viewing(true)).await?;
      write(&mut connection.writer, &Message::Redraw)
        .await
        .context("session ended while switching")?;
    }
  }
  drop(connection);
  drop(terminal);
  if lost {
    bail!("lost the session connection; ainz sessions --live shows its status");
  }
  if let Some(info) = ended
    && let Some(problem) = info.problem
  {
    bail!("{problem}");
  }
  if detached {
    println!(
      "Detached. Reconnect with ainz attach {}",
      &id.to_string()[28..]
    );
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn wide_names_are_clipped_to_terminal_cells_and_control_sequences_are_removed() {
    assert_eq!(clipped("界界reviewer", 5), "界界r");
    assert_eq!(clipped("review\x1b\x07er", 8), "reviewer");
  }

  #[test]
  fn searching_a_name_does_not_match_a_subsequence_spanning_an_unrelated_workspace_path() {
    let rows: Vec<Info> = ["reviewer", "builder"]
      .into_iter()
      .map(|title| {
        serde_json::from_value(serde_json::json!({
          "id": Uuid::now_v7(), "workspace": "/private/var/folders/ainz/work/repo",
          "title": title, "state": "ready", "attached": false
        }))
        .unwrap()
      })
      .collect();
    let found = filtered(&rows, "reviewer");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].title, "reviewer");
    assert_eq!(filtered(&rows, "ainz/work").len(), 2);
    assert_eq!(filtered(&rows, "ready").len(), 2);
  }
}
