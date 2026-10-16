/// Where a run is between steps, and what decides the next one. Nothing here does I/O: the
/// driver in `agent.rs` performs each effect and reports back what came of it, so every
/// branch of a run can be tested by handing `step` actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
  pub steps: usize,
  pub max_steps: usize,
  // the last reply asked for no tools, so the run ends unless somebody steers it
  pub settled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
  Begin,
  Replied { calls: usize },
  ToolsDone,
  Steered { messages: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
  // send the context to the provider
  Ask,
  // run the calls the last reply made
  RunTools,
  // take whatever steering has queued up
  Drain,
  Finish,
  // out of steps with work still open; every call has its result, so "continue" carries on
  Pause,
}

impl Turn {
  pub fn new(max_steps: usize) -> Self {
    Self {
      steps: 0,
      max_steps,
      settled: false,
    }
  }
}

pub fn step(turn: &mut Turn, action: Action) -> Effect {
  match action {
    Action::Begin => ask(turn),
    Action::Replied { calls: 0 } => {
      turn.settled = true;
      Effect::Drain
    }
    Action::Replied { .. } => {
      turn.settled = false;
      Effect::RunTools
    }
    Action::ToolsDone => Effect::Drain,
    Action::Steered { messages: 0 } if turn.settled => Effect::Finish,
    Action::Steered { .. } => ask(turn),
  }
}

fn ask(turn: &mut Turn) -> Effect {
  if turn.steps >= turn.max_steps {
    return Effect::Pause;
  }
  turn.steps += 1;
  Effect::Ask
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn a_reply_without_tools_finishes_the_turn() {
    let mut turn = Turn::new(4);
    assert_eq!(step(&mut turn, Action::Begin), Effect::Ask);
    assert_eq!(step(&mut turn, Action::Replied { calls: 0 }), Effect::Drain);
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 0 }),
      Effect::Finish
    );
  }

  #[test]
  fn tool_results_go_back_to_the_model() {
    let mut turn = Turn::new(4);
    step(&mut turn, Action::Begin);
    assert_eq!(
      step(&mut turn, Action::Replied { calls: 2 }),
      Effect::RunTools
    );
    assert_eq!(step(&mut turn, Action::ToolsDone), Effect::Drain);
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 0 }),
      Effect::Ask
    );
    assert_eq!(turn.steps, 2);
  }

  #[test]
  fn steering_after_a_final_reply_asks_again() {
    let mut turn = Turn::new(4);
    step(&mut turn, Action::Begin);
    step(&mut turn, Action::Replied { calls: 0 });
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 1 }),
      Effect::Ask
    );
  }

  #[test]
  fn running_out_of_steps_pauses_rather_than_fails() {
    let mut turn = Turn::new(1);
    step(&mut turn, Action::Begin);
    step(&mut turn, Action::Replied { calls: 1 });
    step(&mut turn, Action::ToolsDone);
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 0 }),
      Effect::Pause
    );
  }

  #[test]
  fn a_final_reply_on_the_last_step_still_finishes() {
    let mut turn = Turn::new(1);
    step(&mut turn, Action::Begin);
    step(&mut turn, Action::Replied { calls: 0 });
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 0 }),
      Effect::Finish
    );
  }
}
