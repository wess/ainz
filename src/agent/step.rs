/// Where a run is between steps, and what decides the next one. Nothing here does I/O: the
/// driver in `agent.rs` performs each effect and reports back what came of it, so every
/// branch of a run can be tested by handing `step` actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Turn {
  pub steps: usize,
  pub max_steps: usize,
  // the last reply asked for no tools, so the run ends unless somebody steers it
  pub settled: bool,
  // the last reply stopped at its output limit, so it is not the answer even without calls
  pub cut_off: bool,
  // continuations asked for in a row without a call that did anything
  pub continuations: usize,
}

/// How many cut-off replies in a row are carried on from automatically before the run pauses
/// for the person. A model that keeps hitting the limit without getting any work done is not
/// going to get further by being told to continue a fourth time.
pub const CONTINUATIONS: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
  Begin,
  Replied { calls: usize, truncated: bool },
  // `progress` is whether any call ran and succeeded
  ToolsDone { progress: bool },
  Steered { messages: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
  // send the context to the provider
  Ask,
  // send it again, telling the model its last reply was cut off and to carry on from there
  Continue,
  // run the calls the last reply made
  RunTools,
  // take whatever steering has queued up
  Drain,
  Finish,
  // out of steps with work still open; every call has its result, so "continue" carries on
  Pause,
  // cut off too many times in a row without progress; the person decides what happens next
  Stall,
}

impl Turn {
  pub fn new(max_steps: usize) -> Self {
    Self {
      steps: 0,
      max_steps,
      settled: false,
      cut_off: false,
      continuations: 0,
    }
  }
}

pub fn step(turn: &mut Turn, action: Action) -> Effect {
  match action {
    Action::Begin => ask(turn),
    Action::Replied { calls, truncated } => {
      turn.cut_off = truncated;
      turn.settled = calls == 0 && !truncated;
      match calls {
        0 => Effect::Drain,
        _ => Effect::RunTools,
      }
    }
    Action::ToolsDone { progress } => {
      if progress {
        turn.continuations = 0;
      }
      Effect::Drain
    }
    // steering takes the place of carrying on: the person has said what comes next
    Action::Steered { messages } if messages > 0 => {
      turn.continuations = 0;
      turn.cut_off = false;
      ask(turn)
    }
    Action::Steered { .. } if turn.settled => Effect::Finish,
    Action::Steered { .. } if turn.cut_off => continuation(turn),
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

fn continuation(turn: &mut Turn) -> Effect {
  if turn.continuations >= CONTINUATIONS {
    return Effect::Stall;
  }
  match ask(turn) {
    Effect::Ask => {
      turn.continuations += 1;
      Effect::Continue
    }
    other => other,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn replied(calls: usize) -> Action {
    Action::Replied {
      calls,
      truncated: false,
    }
  }

  fn cut_off(calls: usize) -> Action {
    Action::Replied {
      calls,
      truncated: true,
    }
  }

  const QUIET: Action = Action::Steered { messages: 0 };

  #[test]
  fn a_reply_without_tools_finishes_the_turn() {
    let mut turn = Turn::new(4);
    assert_eq!(step(&mut turn, Action::Begin), Effect::Ask);
    assert_eq!(step(&mut turn, replied(0)), Effect::Drain);
    assert_eq!(step(&mut turn, QUIET), Effect::Finish);
  }

  #[test]
  fn tool_results_go_back_to_the_model() {
    let mut turn = Turn::new(4);
    step(&mut turn, Action::Begin);
    assert_eq!(step(&mut turn, replied(2)), Effect::RunTools);
    assert_eq!(
      step(&mut turn, Action::ToolsDone { progress: true }),
      Effect::Drain
    );
    assert_eq!(step(&mut turn, QUIET), Effect::Ask);
    assert_eq!(turn.steps, 2);
  }

  #[test]
  fn steering_after_a_final_reply_asks_again() {
    let mut turn = Turn::new(4);
    step(&mut turn, Action::Begin);
    step(&mut turn, replied(0));
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 1 }),
      Effect::Ask
    );
  }

  #[test]
  fn running_out_of_steps_pauses_rather_than_fails() {
    let mut turn = Turn::new(1);
    step(&mut turn, Action::Begin);
    step(&mut turn, replied(1));
    step(&mut turn, Action::ToolsDone { progress: true });
    assert_eq!(step(&mut turn, QUIET), Effect::Pause);
  }

  #[test]
  fn a_final_reply_on_the_last_step_still_finishes() {
    let mut turn = Turn::new(1);
    step(&mut turn, Action::Begin);
    step(&mut turn, replied(0));
    assert_eq!(step(&mut turn, QUIET), Effect::Finish);
  }

  #[test]
  fn a_text_reply_cut_off_carries_on_instead_of_finishing() {
    let mut turn = Turn::new(8);
    step(&mut turn, Action::Begin);
    assert_eq!(step(&mut turn, cut_off(0)), Effect::Drain);
    assert_eq!(step(&mut turn, QUIET), Effect::Continue);
    assert_eq!(turn.steps, 2);
  }

  #[test]
  fn a_cut_off_reply_with_calls_runs_them_and_then_carries_on() {
    let mut turn = Turn::new(8);
    step(&mut turn, Action::Begin);
    assert_eq!(step(&mut turn, cut_off(2)), Effect::RunTools);
    step(&mut turn, Action::ToolsDone { progress: true });
    assert_eq!(step(&mut turn, QUIET), Effect::Continue);
  }

  #[test]
  fn repeated_cutoffs_without_progress_stall_after_the_limit() {
    let mut turn = Turn::new(32);
    step(&mut turn, Action::Begin);
    for _ in 0..CONTINUATIONS {
      step(&mut turn, cut_off(0));
      assert_eq!(step(&mut turn, QUIET), Effect::Continue);
    }
    step(&mut turn, cut_off(0));
    assert_eq!(step(&mut turn, QUIET), Effect::Stall);
  }

  #[test]
  fn successful_tool_work_resets_the_count() {
    let mut turn = Turn::new(32);
    step(&mut turn, Action::Begin);
    for _ in 0..CONTINUATIONS {
      step(&mut turn, cut_off(0));
      step(&mut turn, QUIET);
    }
    step(&mut turn, cut_off(1));
    step(&mut turn, Action::ToolsDone { progress: true });
    assert_eq!(step(&mut turn, QUIET), Effect::Continue);
    assert_eq!(turn.continuations, 1);
  }

  #[test]
  fn a_failed_call_is_not_progress() {
    let mut turn = Turn::new(32);
    step(&mut turn, Action::Begin);
    for _ in 0..CONTINUATIONS {
      step(&mut turn, cut_off(1));
      step(&mut turn, Action::ToolsDone { progress: false });
      step(&mut turn, QUIET);
    }
    step(&mut turn, cut_off(1));
    step(&mut turn, Action::ToolsDone { progress: false });
    assert_eq!(step(&mut turn, QUIET), Effect::Stall);
  }

  #[test]
  fn steering_after_a_cutoff_takes_precedence() {
    let mut turn = Turn::new(8);
    step(&mut turn, Action::Begin);
    step(&mut turn, cut_off(0));
    assert_eq!(
      step(&mut turn, Action::Steered { messages: 1 }),
      Effect::Ask
    );
    assert_eq!(turn.continuations, 0);
  }

  #[test]
  fn cutoffs_still_respect_the_step_limit() {
    let mut turn = Turn::new(1);
    step(&mut turn, Action::Begin);
    step(&mut turn, cut_off(0));
    assert_eq!(step(&mut turn, QUIET), Effect::Pause);
  }
}
