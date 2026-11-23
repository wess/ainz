//! `/goal`: an objective the session keeps working toward across turns. Each turn that ends
//! without the model saying the goal is met, or that it cannot go on, is followed by another,
//! up to a ceiling, so a long job runs unattended without running for ever.

/// How many turns a goal gets before it pauses for the person.
pub(super) const ROUNDS: usize = 25;

const MET: &str = "GOAL MET";
const BLOCKED: &str = "GOAL BLOCKED";

pub(super) struct Goal {
  pub text: String,
  pub worker: Option<ainz::routing::ModelTarget>,
  /// Turns taken so far, the opening one included.
  pub rounds: usize,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
  Met,
  Blocked(String),
  Open,
}

/// What comes of a `/goal` command or of a turn ending with a goal open.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Next {
  /// Nothing to run; a line for the transcript.
  Note(String),
  /// Run this prompt, showing `shown` in the transcript in its place.
  Run { prompt: String, shown: String },
}

fn opening(goal: &str) -> String {
  format!(
    "Your goal for this session:\n\n{goal}\n\nWork toward it until it is done, verifying as \
     you go. You will be prompted to continue after each turn, so it is fine to stop at a \
     natural checkpoint. When the goal is fully achieved and verified, end your reply with a \
     line reading exactly `{MET}`. If you cannot make progress without the person, end with \
     `{BLOCKED}: ` and the reason. Do not write either line otherwise."
  )
}

fn nudge(goal: &Goal) -> String {
  format!(
    "Continue toward the goal: {}\n\nThis is turn {} of at most {ROUNDS}. Pick up where you \
     left off. End with `{MET}` once it is achieved and verified, or `{BLOCKED}: ` and the \
     reason if you need the person.",
    goal.text,
    goal.rounds + 1
  )
}

/// Read from the last lines only, so a reply that quotes the instructions back is not taken
/// for one that finished.
fn verdict(reply: &str) -> Verdict {
  for line in reply.lines().rev().take(3).map(str::trim) {
    let line = line.trim_matches(|ch| ch == '`' || ch == '*');
    if line == MET {
      return Verdict::Met;
    }
    if let Some(reason) = line.strip_prefix(BLOCKED) {
      return Verdict::Blocked(reason.trim_start_matches(':').trim().to_string());
    }
  }
  Verdict::Open
}

fn advance(goal: &mut Goal) -> Next {
  let prompt = nudge(goal);
  goal.rounds += 1;
  Next::Run {
    prompt,
    shown: format!("goal · turn {} of {ROUNDS}", goal.rounds),
  }
}

/// `/goal TEXT` sets one and starts on it, `/goal` says what it is, `/goal continue` picks a
/// paused one up, and `/goal clear` drops it.
pub(super) fn command(goal: &mut Option<Goal>, args: &str) -> Next {
  let note = |text: &str| Next::Note(text.into());
  match args.trim() {
    "" => match goal {
      Some(current) => Next::Note(format!(
        "goal: {} · turn {} of {ROUNDS}",
        current.text, current.rounds
      )),
      None => note("no goal · /goal TEXT sets one and starts on it"),
    },
    "clear" | "off" | "stop" => {
      *goal = None;
      note("goal cleared")
    }
    "continue" | "resume" => match goal {
      None => note("no goal to continue · /goal TEXT sets one"),
      Some(current) => {
        // a goal that ran out of turns gets a fresh allowance; a blocked one keeps its count
        if current.rounds >= ROUNDS {
          current.rounds = 0;
        }
        advance(current)
      }
    },
    objective => {
      *goal = Some(Goal {
        text: objective.to_string(),
        worker: None,
        rounds: 1,
      });
      Next::Run {
        prompt: opening(objective),
        shown: format!("goal: {objective}"),
      }
    }
  }
}

/// After a turn with a goal open: stop if it is met or blocked, pause if the run did not
/// finish, if the person has started typing, or if the turns ran out, and otherwise go again.
pub(super) fn after(goal: &mut Option<Goal>, reply: Option<&str>, typing: bool) -> Option<Next> {
  let current = goal.as_mut()?;
  let Some(reply) = reply else {
    return Some(Next::Note(
      "goal paused · /goal continue picks it up again".into(),
    ));
  };
  match verdict(reply) {
    Verdict::Met => {
      *goal = None;
      return Some(Next::Note("goal met".into()));
    }
    Verdict::Blocked(reason) => {
      return Some(Next::Note(format!(
        "goal blocked: {reason} · /goal continue once it is sorted, /goal clear drops it"
      )));
    }
    Verdict::Open => {}
  }
  if typing {
    return Some(Next::Note(
      "goal paused for what you typed · /goal continue picks it up again".into(),
    ));
  }
  if current.rounds >= ROUNDS {
    return Some(Next::Note(format!(
      "goal paused after {} turns · /goal continue gives it {ROUNDS} more",
      current.rounds
    )));
  }
  Some(advance(current))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn open(rounds: usize) -> Option<Goal> {
    Some(Goal {
      text: "ship it".into(),
      worker: None,
      rounds,
    })
  }

  #[test]
  fn a_reply_ending_with_the_marker_meets_the_goal() {
    assert_eq!(verdict("all tests pass\n\n**GOAL MET**"), Verdict::Met);
  }

  #[test]
  fn a_blocked_reply_carries_its_reason() {
    assert_eq!(
      verdict("tried twice\nGOAL BLOCKED: needs the staging password"),
      Verdict::Blocked("needs the staging password".into())
    );
  }

  #[test]
  fn a_marker_quoted_early_in_a_long_reply_does_not_end_the_goal() {
    let reply = "I will write GOAL MET when done.\nstep one\nstep two\nstep three\nstep four";
    assert_eq!(verdict(reply), Verdict::Open);
  }

  #[test]
  fn setting_a_goal_starts_on_it() {
    let mut goal = None;
    let Next::Run { prompt, shown } = command(&mut goal, "make the tests pass") else {
      panic!("a new goal runs");
    };
    assert!(prompt.contains("make the tests pass"));
    assert!(prompt.contains(MET));
    assert_eq!(shown, "goal: make the tests pass");
    assert_eq!(goal.unwrap().rounds, 1);
  }

  #[test]
  fn an_open_turn_is_followed_by_the_next_one() {
    let mut goal = open(3);
    let Some(Next::Run { prompt, shown }) = after(&mut goal, Some("did a bit"), false) else {
      panic!("an open goal carries on");
    };
    assert!(prompt.contains("turn 4 of"));
    assert_eq!(shown, format!("goal · turn 4 of {ROUNDS}"));
    assert_eq!(goal.unwrap().rounds, 4);
  }

  #[test]
  fn a_met_goal_is_dropped() {
    let mut goal = open(2);
    assert_eq!(
      after(&mut goal, Some("done\nGOAL MET"), false),
      Some(Next::Note("goal met".into()))
    );
    assert!(goal.is_none());
  }

  #[test]
  fn a_failed_turn_a_draft_or_the_ceiling_pauses_it() {
    for (reply, typing, rounds) in [
      (None, false, 2),
      (Some("working"), true, 2),
      (Some("working"), false, ROUNDS),
    ] {
      let mut goal = open(rounds);
      assert!(matches!(
        after(&mut goal, reply, typing),
        Some(Next::Note(note)) if note.contains("paused")
      ));
      assert!(goal.is_some());
    }
  }

  #[test]
  fn continuing_after_the_ceiling_gives_it_a_fresh_allowance() {
    let mut goal = open(ROUNDS);
    assert!(matches!(command(&mut goal, "continue"), Next::Run { .. }));
    assert_eq!(goal.unwrap().rounds, 1);
  }

  #[test]
  fn no_goal_means_nothing_happens_after_a_turn() {
    assert_eq!(after(&mut None, Some("anything"), false), None);
  }
}
