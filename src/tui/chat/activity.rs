use super::*;

#[derive(Clone, Copy)]
pub(super) enum RunStatus {
  Completed,
  Cancelled,
  Failed,
}

impl RunStatus {
  fn label(self) -> (&'static str, Color) {
    match self {
      Self::Completed => ("✓ Completed", ACTIVE),
      Self::Cancelled => ("■ Cancelled", YELLOW),
      Self::Failed => ("✗ Failed", RED),
    }
  }
}

impl ChatState {
  pub(super) fn begin_run(&mut self) {
    self.started = Some(Instant::now());
    self.outcome = None;
    self.cancelled = false;
    self.primary.refresh_topic();
    self.primary.finish_message();
    self.primary.state = AgentState::Running;
    self.scroll.set(0);
  }

  pub(super) fn finish_run(&mut self, result: Result<String>) {
    let took = self
      .started
      .take()
      .map(|start| start.elapsed())
      .unwrap_or_default();
    let status = if self.cancelled {
      RunStatus::Cancelled
    } else if result.is_err() {
      RunStatus::Failed
    } else {
      RunStatus::Completed
    };
    if let Err(error) = result
      && !self.cancelled
    {
      self
        .primary
        .entries
        .push(Entry::new(EntryKind::Error, format!("{error:#}")));
    }
    for (index, _) in self.primary.live.values() {
      if let Some(tool) = self
        .primary
        .entries
        .get_mut(*index)
        .and_then(|entry| entry.tool.as_mut())
      {
        tool.state = ToolState::Stopped;
        tool.note = "stopped before a result was returned".into();
        tool.live.clear();
      }
    }
    self.primary.live.clear();
    self.primary.tools.clear();
    self.primary.finish_message();
    self.primary.state = match status {
      RunStatus::Completed => AgentState::Done,
      _ => AgentState::Error,
    };
    let kind = match status {
      RunStatus::Completed => EntryKind::Completed,
      RunStatus::Cancelled => EntryKind::Cancelled,
      RunStatus::Failed => EntryKind::Error,
    };
    let (label, _) = status.label();
    self.primary.entries.push(Entry::new(
      kind,
      format!("{label} · {}", elapsed(took.as_secs())),
    ));
    self.outcome = Some((status, took));
    self.cancelled = false;
    self.approval = None;
  }
}

pub(super) fn render(frame: &mut Frame, area: Rect, state: &ChatState) {
  let (label, color) = if state.active.is_some() {
    match state.active_view().state {
      AgentState::Running => ("● Working".into(), YELLOW),
      AgentState::Done => ("✓ Completed".into(), ACTIVE),
      AgentState::Error => ("✗ Failed".into(), RED),
    }
  } else if let Some(started) = state.started {
    let phase = if state.cancelled {
      "Cancelling"
    } else if state.approval.is_some() {
      "Waiting for approval"
    } else if !state.primary.tools.is_empty() {
      "Running tools"
    } else if state.primary.assistant.is_some() {
      "Responding"
    } else {
      "Working"
    };
    (
      format!("● {phase} · {}", elapsed(started.elapsed().as_secs())),
      YELLOW,
    )
  } else if let Some((status, took)) = state.outcome {
    let (label, color) = status.label();
    (format!("{label} · {}", elapsed(took.as_secs())), color)
  } else {
    ("Ready".into(), MUTED)
  };
  let hint = if state.header_preview {
    "  ·  Header preview · any key returns"
  } else if state.scroll.get() > 0 {
    "  ·  History · Ctrl+End latest"
  } else if state.active.is_some() {
    "  ·  Ctrl+1 main"
  } else if state.busy() {
    "  ·  Esc cancel"
  } else {
    "  ·  Enter to send"
  };
  frame.render_widget(
    Paragraph::new(Line::from(vec![
      Span::styled(
        format!(" {label}"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
      ),
      Span::styled(hint, Style::default().fg(MUTED)),
    ])),
    area,
  );
}
