use std::collections::VecDeque;

use super::*;
use crate::control::cancelled;

#[derive(Default)]
pub(super) struct RunState {
  pub usage: Usage,
  pub pending: VecDeque<ToolCall>,
  cancel: Option<tokio::sync::watch::Receiver<bool>>,
}

impl RunState {
  fn cancelled(&self) -> bool {
    self.cancel.as_ref().is_some_and(|signal| *signal.borrow())
  }

  pub(super) fn checkpoint(&self) -> Result<()> {
    if self.cancelled() {
      bail!("run cancelled");
    }
    Ok(())
  }
}

impl<P: ChatProvider> Agent<P> {
  pub(super) async fn run_message(
    &self,
    session: &mut Session,
    prompt: Message,
    options: RunOptions,
    mut inbox: Option<&mut RunInbox>,
  ) -> Result<String> {
    options.validate()?;
    let signal = inbox.as_ref().map(|inbox| inbox.cancellation());
    let mut state = RunState {
      cancel: signal.clone(),
      ..Default::default()
    };
    let (result, interrupted) = {
      let execution = async {
        if session.nodes.is_empty() {
          options
            .hooks
            .session_start(&self.workspace, session.id, &self.events)
            .await;
        }
        state.checkpoint()?;
        let result = self
          .run_turn(session, prompt, &options, inbox.as_deref_mut(), &mut state)
          .await;
        if let Some(inbox) = inbox.as_deref_mut() {
          inbox.close();
        }
        state.checkpoint()?;
        options
          .hooks
          .session_end(
            &self.workspace,
            session.id,
            result.as_deref().ok(),
            result.is_err(),
            &self.events,
          )
          .await;
        result
      };
      tokio::pin!(execution);
      tokio::select! {
        biased;
        () = cancelled(signal) => (Err(anyhow::anyhow!("run cancelled")), true),
        result = &mut execution => (result, false),
      }
    };
    if let Some(inbox) = inbox {
      inbox.close();
    }
    let interrupted = interrupted || state.cancelled();
    let result = if interrupted {
      Err(anyhow::anyhow!("run cancelled"))
    } else {
      result
    };
    // the execution future and its process guards are dropped before terminal events are sent
    accumulate(&mut session.usage, &state.usage);
    for call in state.pending {
      let output = "tool call cancelled; any partial effects may already have occurred".to_string();
      session.append(Message::tool(call.id.clone(), output.clone()));
      self.events.emit(Event::ToolEnd {
        id: call.id,
        output,
        error: true,
      });
    }
    if interrupted {
      self.events.emit(Event::Cancelled);
    } else if result.is_ok() {
      self.events.emit(Event::TurnEnd { usage: state.usage });
    }
    result
  }
}
