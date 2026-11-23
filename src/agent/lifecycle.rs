use std::collections::{HashMap, VecDeque};

use super::*;
use crate::control::cancelled;

#[derive(Default)]
pub(super) struct RunState {
  pub usage: Usage,
  pub target: Option<ModelTarget>,
  pub pending: VecDeque<ToolCall>,
  // results that came back before they were written to the session: a cancel that lands
  // while a batch is running keeps the calls that did finish
  pub finished: HashMap<String, (String, bool, bool)>,
  // the open plan, kept honest by a reminder when the model stops updating it
  pub planning: bool,
  pub plan: Option<super::plan::Plan>,
  // who answers for a tool the provider runs itself; built once for the run
  pub asks: Option<Approver>,
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
        let result = match self.worker(session, &prompt, &options, &mut state).await {
          Ok(worker) => {
            worker
              .as_ref()
              .unwrap_or(self)
              .run_turn(session, prompt, &options, inbox.as_deref_mut(), &mut state)
              .await
          }
          Err(error) => Err(error),
        };
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
      if let Some((output, ..)) = state.finished.remove(&call.id) {
        session.append(Message::tool(call.id, output));
        continue;
      }
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

  async fn worker(
    &self,
    session: &Session,
    prompt: &Message,
    options: &RunOptions,
    state: &mut RunState,
  ) -> Result<Option<Self>> {
    let Some(selector) = &self.selector else {
      return Ok(None);
    };
    let choice = selector
      .select(session, prompt, options.continue_with.as_ref())
      .await?;
    state.usage = choice.usage;
    if let Some((strategy, reason)) = choice.decision {
      self.events.emit(Event::ModelSelected {
        target: choice.target.clone(),
        strategy,
        reason,
      });
    }
    state.target = Some(choice.target);
    Ok(choice.provider.map(|provider| {
      Agent::new(
        provider,
        self.tools.clone(),
        self.workspace.clone(),
        self.events.clone(),
        self.approver.clone(),
      )
    }))
  }
}
