use agentx::command_palette::{SlashCommand, builtins, matches};

#[test]
fn slash_search_prioritizes_exact_prefix_and_fuzzy_matches() {
  let commands = builtins();

  assert_eq!(matches(&commands, "/sta")[0].name, "status");
  assert_eq!(matches(&commands, "/stts")[0].name, "status");
  assert_eq!(matches(&commands, "/token")[0].name, "usage");
  assert!(matches(&commands, "status").is_empty());
  assert!(matches(&commands, "/image ").is_empty());
}

#[test]
fn commands_with_arguments_complete_with_a_space() {
  let command = SlashCommand::new(
    "image",
    "/image <PATH> <PROMPT>",
    "Attach an image",
    "prompt",
  );

  assert_eq!(command.completion(), "/image ");
  assert_eq!(
    builtins()
      .iter()
      .find(|item| item.name == "status")
      .unwrap()
      .completion(),
    "/status"
  );
}
