//! Named interface actions, portable key notation, and chord resolution.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Action {
  Detach,
  Sessions,
  NewSession,
  NextSession,
  PreviousSession,
  NextAttention,
  Settings,
  Interrupt,
  Quit,
  Send,
  Newline,
  Clear,
  Roster,
  Detail,
  NextAgent,
  PreviousAgent,
  ScrollUp,
  ScrollDown,
  Permissions,
}

impl Action {
  pub const ALL: [Self; 19] = [
    Self::Detach,
    Self::Sessions,
    Self::NewSession,
    Self::NextSession,
    Self::PreviousSession,
    Self::NextAttention,
    Self::Settings,
    Self::Interrupt,
    Self::Quit,
    Self::Send,
    Self::Newline,
    Self::Clear,
    Self::Roster,
    Self::Detail,
    Self::NextAgent,
    Self::PreviousAgent,
    Self::ScrollUp,
    Self::ScrollDown,
    Self::Permissions,
  ];
  pub fn label(self) -> &'static str {
    match self {
      Self::Detach => "detach",
      Self::Sessions => "switch sessions",
      Self::NewSession => "new session",
      Self::NextSession => "next session",
      Self::PreviousSession => "previous session",
      Self::NextAttention => "next session needing attention",
      Self::Settings => "open settings",
      Self::Interrupt => "cancel / clear",
      Self::Quit => "quit",
      Self::Send => "send prompt",
      Self::Newline => "new line",
      Self::Clear => "clear input",
      Self::Roster => "agent roster",
      Self::Detail => "tool details",
      Self::NextAgent => "next agent",
      Self::PreviousAgent => "previous agent",
      Self::ScrollUp => "scroll up",
      Self::ScrollDown => "scroll down",
      Self::Permissions => "permission mode",
    }
  }
  pub fn default_key(self) -> &'static str {
    match self {
      Self::Detach => "ctrl+b d",
      Self::Sessions => "ctrl+b s",
      Self::NewSession => "ctrl+b n",
      Self::NextSession => "ctrl+b ]",
      Self::PreviousSession => "ctrl+b [",
      Self::NextAttention => "ctrl+b a",
      Self::Settings => "f2",
      Self::Interrupt => "ctrl+c",
      Self::Quit => "ctrl+d",
      Self::Send => "enter",
      Self::Newline => "shift+enter",
      Self::Clear => "ctrl+u",
      Self::Roster => "ctrl+l",
      Self::Detail => "ctrl+o",
      Self::NextAgent => "ctrl+=",
      Self::PreviousAgent => "ctrl+-",
      Self::ScrollUp => "pageup",
      Self::ScrollDown => "pagedown",
      Self::Permissions => "shift+tab",
    }
  }
  pub fn live(self) -> bool {
    matches!(
      self,
      Self::Detach
        | Self::Sessions
        | Self::NewSession
        | Self::NextSession
        | Self::PreviousSession
        | Self::NextAttention
    )
  }
  pub fn help(self) -> &'static str {
    match self {
      Self::Detach => {
        "Return to the shell and leave the agent running. Requires background sessions."
      }
      Self::Sessions => "Choose another live session. Requires background sessions.",
      Self::NextAttention => {
        "Visit approvals, then unseen failures and completions. Requires background sessions."
      }
      Self::NewSession => "Start another session in this workspace. Requires background sessions.",
      Self::NextSession | Self::PreviousSession => {
        "Cycle live sessions. Requires background sessions."
      }
      Self::Interrupt => "Cancel a run, or exit an idle session. Esc remains available.",
      Self::Quit => "End an empty, idle session.",
      Self::Send => "Send the prompt, or accept the selected completion.",
      Self::Permissions => {
        "Cycle Ask, Auto, and Read-only. This changes which tools need approval."
      }
      _ => "Press Enter to record a shortcut. Changes apply immediately and are saved.",
    }
  }
}

pub type Overrides = BTreeMap<Action, String>;
pub fn label(overrides: &Overrides, action: Action) -> &str {
  overrides
    .get(&action)
    .map(String::as_str)
    .unwrap_or(action.default_key())
}

#[cfg(feature = "cli")]
pub use terminal::*;
#[cfg(feature = "cli")]
mod terminal {
  use super::*;
  use anyhow::{Result, bail};
  use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
  pub fn normalize(key: KeyEvent) -> KeyEvent {
    let mut modifiers = key.modifiers
      & (KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::SUPER);
    let code = match key.code {
      KeyCode::BackTab => {
        modifiers |= KeyModifiers::SHIFT;
        KeyCode::Tab
      }
      KeyCode::Char(ch) => KeyCode::Char(ch.to_ascii_lowercase()),
      code => code,
    };
    KeyEvent::new(code, modifiers)
  }

  pub fn format(key: KeyEvent) -> Result<String> {
    let key = normalize(key);
    let mut text = String::new();
    for (modifier, name) in [
      (KeyModifiers::CONTROL, "ctrl+"),
      (KeyModifiers::ALT, "alt+"),
      (KeyModifiers::SHIFT, "shift+"),
      (KeyModifiers::SUPER, "super+"),
    ] {
      if key.modifiers.contains(modifier) {
        text.push_str(name);
      }
    }
    text.push_str(&match key.code {
      KeyCode::Char(' ') => "space".into(),
      KeyCode::Char('+') => "plus".into(),
      KeyCode::Char(',') => "comma".into(),
      KeyCode::Char(ch) => ch.to_string(),
      KeyCode::F(n) if (1..=12).contains(&n) => format!("f{n}"),
      KeyCode::Enter => "enter".into(),
      KeyCode::Tab => "tab".into(),
      KeyCode::Esc => "esc".into(),
      KeyCode::PageUp => "pageup".into(),
      KeyCode::PageDown => "pagedown".into(),
      KeyCode::Up => "up".into(),
      KeyCode::Down => "down".into(),
      KeyCode::Left => "left".into(),
      KeyCode::Right => "right".into(),
      KeyCode::Home => "home".into(),
      KeyCode::End => "end".into(),
      KeyCode::Delete => "delete".into(),
      KeyCode::Backspace => "backspace".into(),
      _ => bail!("choose a letter, number, navigation key, or F1–F12"),
    });
    Ok(text)
  }

  pub fn parse(text: &str) -> Result<Vec<KeyEvent>> {
    if text.eq_ignore_ascii_case("none") {
      return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for word in text.split_whitespace() {
      let word = if word.ends_with(',') && !word.ends_with("+,") && word != "," {
        &word[..word.len() - 1]
      } else {
        word
      }
      .to_ascii_lowercase();
      let parts: Vec<_> = word.split('+').collect();
      let mut modifiers = KeyModifiers::NONE;
      for part in &parts[..parts.len() - 1] {
        modifiers |= match *part {
          "ctrl" | "control" => KeyModifiers::CONTROL,
          "alt" | "option" => KeyModifiers::ALT,
          "shift" => KeyModifiers::SHIFT,
          "super" | "cmd" | "command" => KeyModifiers::SUPER,
          _ => bail!("unknown modifier {part}; use Ctrl, Alt, Shift, or Super"),
        };
      }
      let key = parts[parts.len() - 1];
      let code = match key {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "comma" => KeyCode::Char(','),
        "plus" => KeyCode::Char('+'),
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "delete" => KeyCode::Delete,
        "backspace" => KeyCode::Backspace,
        value
          if value.starts_with('f')
            && value[1..]
              .parse::<u8>()
              .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
          KeyCode::F(value[1..].parse().unwrap())
        }
        value if value.chars().count() == 1 => KeyCode::Char(value.chars().next().unwrap()),
        _ => bail!("unknown key {key}; try Ctrl+X, Alt+D, or F6"),
      };
      result.push(normalize(KeyEvent::new(code, modifiers)));
    }
    if !(1..=2).contains(&result.len()) {
      bail!("use one key combination or a two-key chord, such as Ctrl+B D");
    }
    Ok(result)
  }

  pub fn validate(overrides: &Overrides) -> Result<()> {
    let mut shortcuts: Vec<(Action, Vec<KeyEvent>)> = Vec::new();
    for action in Action::ALL {
      let keys = parse(label(overrides, action))?;
      if keys.is_empty() && matches!(action, Action::Send | Action::Interrupt | Action::Quit) {
        bail!("{} must have a shortcut", action.label());
      }
      if let Some(first) = keys.first() {
        let modified = first
          .modifiers
          .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER);
        let navigation = first.modifiers.contains(KeyModifiers::SHIFT)
          && matches!(
            first.code,
            KeyCode::Enter
              | KeyCode::Tab
              | KeyCode::Home
              | KeyCode::End
              | KeyCode::Up
              | KeyCode::Down
              | KeyCode::Left
              | KeyCode::Right
          );
        let safe = modified
          || navigation
          || matches!(
            first.code,
            KeyCode::F(_) | KeyCode::PageUp | KeyCode::PageDown
          )
          || (action == Action::Send && first.code == KeyCode::Enter);
        if !safe || first.code == KeyCode::Esc {
          bail!(
            "{}: start with Ctrl, Alt, Super, or a function key; Esc and plain typing stay available",
            action.label()
          );
        }
        for (other, previous) in &shortcuts {
          if !previous.is_empty() && (previous.starts_with(&keys) || keys.starts_with(previous)) {
            bail!(
              "{} conflicts with {} ({})",
              action.label(),
              other.label(),
              label(overrides, *other)
            );
          }
        }
      }
      shortcuts.push((action, keys));
    }
    Ok(())
  }

  pub fn set(overrides: &mut Overrides, action: Action, text: &str) -> Result<()> {
    let keys = parse(text)?;
    let canonical = if keys.is_empty() {
      "none".into()
    } else {
      keys
        .into_iter()
        .map(format)
        .collect::<Result<Vec<_>>>()?
        .join(" ")
    };
    let mut updated = overrides.clone();
    if canonical == action.default_key() {
      updated.remove(&action);
    } else {
      updated.insert(action, canonical);
    }
    validate(&updated)?;
    *overrides = updated;
    Ok(())
  }

  #[derive(Default)]
  pub struct Resolver {
    pending: Option<KeyEvent>,
  }
  #[derive(Debug, PartialEq, Eq)]
  pub enum Match {
    Action(Action),
    Prefix,
    None,
  }
  impl Resolver {
    pub fn clear(&mut self) {
      self.pending = None;
    }
    pub fn resolve(&mut self, overrides: &Overrides, key: KeyEvent, modal: bool) -> Match {
      let key = normalize(key);
      let previous = self.pending.take();
      for action in Action::ALL
        .into_iter()
        .filter(|action| !modal || action.live() || *action == Action::Interrupt)
      {
        let Ok(keys) = parse(label(overrides, action)) else {
          continue;
        };
        if let Some(first) = previous
          && keys.as_slice() == [first, key]
        {
          return Match::Action(action);
        }
      }
      for action in Action::ALL
        .into_iter()
        .filter(|action| !modal || action.live() || *action == Action::Interrupt)
      {
        let Ok(keys) = parse(label(overrides, action)) else {
          continue;
        };
        if keys.first() == Some(&key) {
          if keys.len() == 1 {
            return Match::Action(action);
          }
          self.pending = Some(key);
          return Match::Prefix;
        }
      }
      Match::None
    }
  }

  #[cfg(test)]
  mod tests {
    use super::*;

    #[test]
    fn punctuation_and_comma_separated_chords_keep_the_key_that_was_chosen() {
      for text in ["ctrl+comma", "ctrl+plus", "ctrl+,", "Ctrl+B, D"] {
        let keys = parse(text).unwrap();
        let canonical = keys
          .iter()
          .copied()
          .map(format)
          .collect::<Result<Vec<_>>>()
          .unwrap()
          .join(" ");
        assert_eq!(parse(&canonical).unwrap(), keys);
      }
      assert_eq!(
        parse("ctrl+,").unwrap(),
        vec![KeyEvent::new(KeyCode::Char(','), KeyModifiers::CONTROL)]
      );
    }

    #[test]
    fn default_shortcuts_do_not_conflict_and_every_one_round_trips() {
      validate(&Overrides::new()).unwrap();
      for action in Action::ALL {
        let keys = parse(action.default_key()).unwrap();
        assert_eq!(
          keys
            .into_iter()
            .map(format)
            .collect::<Result<Vec<_>>>()
            .unwrap()
            .join(" "),
          action.default_key()
        );
      }
    }

    #[test]
    fn conflicts_and_reserved_typing_keys_leave_the_previous_binding_unchanged() {
      let mut keys = Overrides::new();
      for text in [
        "ctrl+c",
        "ctrl+b",
        "d",
        "shift+d",
        "esc",
        "ctrl+oops",
        "f99",
      ] {
        assert!(set(&mut keys, Action::Detach, text).is_err(), "{text}");
        assert!(keys.is_empty());
      }
      set(&mut keys, Action::Detach, "Alt+D").unwrap();
      assert_eq!(label(&keys, Action::Detach), "alt+d");
      assert!(set(&mut keys, Action::Send, "none").is_err());
      assert!(set(&mut keys, Action::Interrupt, "none").is_err());
    }

    #[test]
    fn custom_shortcuts_replace_the_old_keys_and_chords_can_share_a_prefix() {
      let mut keys = Overrides::new();
      set(&mut keys, Action::Detach, "ctrl+x d").unwrap();
      set(&mut keys, Action::Sessions, "ctrl+x s").unwrap();
      let mut resolver = Resolver::default();
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL),
          false
        ),
        Match::Prefix
      );
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
          false
        ),
        Match::None
      );
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
          false
        ),
        Match::Prefix
      );
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
          false
        ),
        Match::Action(Action::Detach)
      );
      set(&mut keys, Action::Detach, Action::Detach.default_key()).unwrap();
      assert!(!keys.contains_key(&Action::Detach));
    }

    #[test]
    fn modal_screens_keep_cancel_and_live_controls_without_intercepting_approval_enter() {
      let mut resolver = Resolver::default();
      let keys = Overrides::new();
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
          true
        ),
        Match::None
      );
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
          true
        ),
        Match::Action(Action::Interrupt)
      );
      assert_eq!(
        resolver.resolve(
          &keys,
          KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
          false
        ),
        Match::Action(Action::Permissions)
      );
    }
  }
}
