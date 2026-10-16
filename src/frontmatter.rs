// the leading `---` block shared by prompt templates and skills: `key: value` lines, then the body

pub(crate) struct FrontMatter<'a> {
  block: &'a str,
  pub(crate) body: &'a str,
}

pub(crate) fn parse(text: &str) -> FrontMatter<'_> {
  let plain = FrontMatter {
    block: "",
    body: text,
  };
  // a file saved on Windows still has front matter; lines() takes the \r off each field
  let Some(rest) = text
    .strip_prefix("---\n")
    .or_else(|| text.strip_prefix("---\r\n"))
  else {
    return plain;
  };
  let Some(end) = rest.find("\n---") else {
    return plain;
  };
  FrontMatter {
    block: &rest[..end],
    body: rest[end + 4..].trim_start(),
  }
}

impl FrontMatter<'_> {
  // a key written as `name:` with nothing after it is as good as absent, so a caller's
  // fallback applies instead of an empty name
  pub(crate) fn field(&self, key: &str) -> Option<String> {
    self
      .block
      .lines()
      .find_map(|line| line.trim_start().strip_prefix(key)?.strip_prefix(':'))
      .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
      .filter(|value| !value.is_empty())
  }
}

#[cfg(test)]
mod tests {
  use super::parse;

  #[test]
  fn a_file_saved_with_windows_line_endings_still_has_front_matter() {
    let front = parse("---\r\nname: demo\r\ndescription: it\r\n---\r\nBody\r\n");
    assert_eq!(front.field("name").as_deref(), Some("demo"));
    assert_eq!(front.field("description").as_deref(), Some("it"));
    assert_eq!(front.body, "Body\r\n");
  }

  #[test]
  fn indented_keys_are_read_and_empty_values_count_as_missing() {
    let front = parse("---\n  name: demo\ndescription:\nquoted: ''\n---\nBody");
    assert_eq!(front.field("name").as_deref(), Some("demo"));
    assert_eq!(front.field("description"), None);
    assert_eq!(front.field("quoted"), None);
  }
}
