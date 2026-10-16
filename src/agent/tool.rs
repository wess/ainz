use std::{collections::VecDeque, panic::AssertUnwindSafe, sync::Arc};

use futures_util::{FutureExt, StreamExt, stream::FuturesUnordered};

use super::{plan, *};
use crate::{
  PermissionMode,
  protocol::INVALID_ARGUMENTS,
  tool::{Tool, ToolContext, truncate},
};

// what a call came to: its output, and whether it failed
pub(super) type Outcome = (String, bool);

enum Admission {
  Run(Arc<dyn Tool>),
  Refused(String),
}

impl<P: ChatProvider> Agent<P> {
  /// Runs the calls in `state.pending`, recording each result in the session in the order the
  /// model made them. Calls that only read run side by side; anything else runs on its own,
  /// in order, because a write and the read after it must not race. Who may run what is
  /// settled one call at a time before anything starts, so an approval prompt never has a
  /// second one racing it.
  pub(super) async fn run_tools(
    &self,
    session: &mut Session,
    options: &RunOptions,
    state: &mut RunState,
  ) -> Result<()> {
    while !state.pending.is_empty() {
      let batch = self.batch(&state.pending);
      let calls: Vec<ToolCall> = state.pending.iter().take(batch).cloned().collect();
      let mut running = FuturesUnordered::new();
      for call in &calls {
        state.checkpoint()?;
        self.events.emit(Event::ToolStart { call: call.clone() });
        match self.admit(call, options, session.id).await {
          Admission::Run(tool) => {
            running.push(self.execute(tool, call, options, session.id));
          }
          Admission::Refused(reason) => self.finish(state, call, (reason, true), false),
        }
      }
      while let Some((id, outcome)) = running.next().await {
        if let Some(call) = calls.iter().find(|call| call.id == id) {
          self.finish(state, call, outcome, true);
        }
      }
      drop(running);
      for call in &calls {
        // the result stays where a cancel can find it until the session has it
        let Some((output, error, executed)) = state.finished.get(&call.id) else {
          continue;
        };
        if *executed {
          options
            .hooks
            .post_tool(
              &self.workspace,
              session.id,
              call,
              output,
              *error,
              &self.events,
            )
            .await;
        }
        if let Some((mut output, ..)) = state.finished.remove(&call.id) {
          // the reminder is for the model; the person already saw the output the tool gave
          if state.planning {
            let (plan, reminder) = plan::observe(state.plan.take(), call);
            state.plan = plan;
            output.push_str(reminder.as_deref().unwrap_or_default());
          }
          session.append(Message::tool(call.id.clone(), output));
        }
        state.pending.pop_front();
      }
      state.checkpoint()?;
    }
    Ok(())
  }

  // how many calls from the front of the queue may run at once
  fn batch(&self, pending: &VecDeque<ToolCall>) -> usize {
    let concurrent = |call: &ToolCall| {
      self
        .tools
        .get(&call.name)
        .is_some_and(|tool| tool.concurrent(&call.arguments))
    };
    pending
      .iter()
      .take_while(|call| concurrent(call))
      .count()
      .max(1)
  }

  fn finish(&self, state: &mut RunState, call: &ToolCall, outcome: Outcome, executed: bool) {
    let (output, error) = outcome;
    self.events.emit(Event::ToolEnd {
      id: call.id.clone(),
      output: output.clone(),
      error,
    });
    state
      .finished
      .insert(call.id.clone(), (output, error, executed));
  }

  async fn admit(
    &self,
    call: &ToolCall,
    options: &RunOptions,
    session_id: uuid::Uuid,
  ) -> Admission {
    let refuse = Admission::Refused;
    let Some(tool) = self.tools.get(&call.name) else {
      return refuse(format!("unknown tool: {}", call.name));
    };
    // a provider keeps a call whose arguments did not parse rather than failing the reply, so
    // the model hears why and can try again; it is nearly always a reply cut off at its length
    // limit in the middle of a large write
    if call.arguments.get(INVALID_ARGUMENTS).is_some() {
      return refuse(
        "the arguments for this call were not valid JSON, most likely because the reply was \
         cut off at its length limit; send it again in smaller pieces"
          .into(),
      );
    }
    if let Err(error) = tool.validate(&call.arguments) {
      return refuse(format!("invalid arguments: {error:#}"));
    }
    let subject = match crate::permission::subject(&self.workspace, call).await {
      Ok(subject) => subject,
      Err(error) => return refuse(format!("{error:#}")),
    };
    let risk = tool.risk(&call.arguments);
    // a standing rule answers before anyone is asked, in every mode: it is the same decision,
    // made once already
    let rules =
      match crate::permission::normalize(&options.rules, &self.workspace, &call.name).await {
        Ok(rules) => rules,
        Err(error) => return refuse(format!("invalid permission rule: {error:#}")),
      };
    let ruled = rules.decide(&call.name, subject.as_deref());
    let allowed = match (ruled, options.permissions) {
      (Some(decided), _) => decided,
      (None, PermissionMode::Auto) => true,
      (None, PermissionMode::ReadOnly) => risk == Risk::Read,
      (None, PermissionMode::Ask) => risk == Risk::Read || (self.approver)(call, risk).await,
    };
    if !allowed {
      return refuse(format!(
        "permission denied for {} ({risk:?}); the user declined it, so do not retry the same \
         call",
        call.name
      ));
    }
    // a call refused above never reaches this, so a pre_tool hook only ever sees work that
    // was already going to run
    if let Err(reason) = options
      .hooks
      .pre_tool(&self.workspace, session_id, call, &self.events)
      .await
    {
      return refuse(reason);
    }
    Admission::Run(tool.clone())
  }

  async fn execute(
    &self,
    tool: Arc<dyn Tool>,
    call: &ToolCall,
    options: &RunOptions,
    session_id: uuid::Uuid,
  ) -> (String, Outcome) {
    let context = ToolContext {
      workspace: self.workspace.clone(),
      session_id,
      max_output_bytes: options.max_output_bytes,
      progress: Some((self.events.clone(), call.id.clone())),
    };
    // a tool that panics fails its call, not the run that holds the session
    let run = AssertUnwindSafe(tool.execute(&context, call.arguments.clone())).catch_unwind();
    let outcome = match run.await {
      Ok(Ok(output)) => (truncate(output, options.max_output_bytes), false),
      Ok(Err(error)) => (format!("{error:#}"), true),
      Err(panic) => (
        format!("{} failed: {}", call.name, panic_message(&*panic)),
        true,
      ),
    };
    (call.id.clone(), outcome)
  }
}

pub(super) fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
  panic
    .downcast_ref::<&str>()
    .map(|text| (*text).to_string())
    .or_else(|| panic.downcast_ref::<String>().cloned())
    .unwrap_or_else(|| "it panicked".into())
}
