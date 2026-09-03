use ainz::synapse::parse_secrets;

#[test]
fn parses_scoped_and_global_rows() {
  let output =
    "apis.DigitalOcean\tDIGITAL_OCEAN_API\tscoped\napis.FishAudio\tFISH_AUDIO_KEY\tglobal\n";
  let secrets = parse_secrets(output);
  assert_eq!(secrets.len(), 2);
  assert_eq!(secrets[0].name, "apis.DigitalOcean");
  assert_eq!(secrets[0].var, "DIGITAL_OCEAN_API");
  assert!(!secrets[0].global);
  assert_eq!(secrets[1].name, "apis.FishAudio");
  assert_eq!(secrets[1].var, "FISH_AUDIO_KEY");
  assert!(secrets[1].global);
}

#[test]
fn skips_blank_lines() {
  let output =
    "apis.DigitalOcean\tDIGITAL_OCEAN_API\tscoped\n\napis.FishAudio\tFISH_AUDIO_KEY\tglobal\n";
  let secrets = parse_secrets(output);
  assert_eq!(secrets.len(), 2);
}

#[test]
fn skips_rows_with_too_few_columns() {
  let output = "apis.DigitalOcean\tDIGITAL_OCEAN_API\tscoped\napis.Bad\tBAD_VAR\napis.FishAudio\tFISH_AUDIO_KEY\tglobal\n";
  let secrets = parse_secrets(output);
  assert_eq!(secrets.len(), 2);
  assert!(secrets.iter().all(|secret| secret.name != "apis.Bad"));
}

#[test]
fn skips_rows_with_an_unknown_scope_word() {
  let output = "apis.DigitalOcean\tDIGITAL_OCEAN_API\tsometimes\n";
  assert!(parse_secrets(output).is_empty());
}

#[test]
fn empty_output_is_no_secrets() {
  assert!(parse_secrets("").is_empty());
}

#[test]
fn tolerates_no_trailing_newline() {
  let output = "apis.FishAudio\tFISH_AUDIO_KEY\tglobal";
  let secrets = parse_secrets(output);
  assert_eq!(secrets.len(), 1);
  assert_eq!(secrets[0].name, "apis.FishAudio");
}
