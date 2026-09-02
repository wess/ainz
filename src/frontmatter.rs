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
  let Some(rest) = text.strip_prefix("---\n") else {
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
  pub(crate) fn field(&self, key: &str) -> Option<String> {
    self
      .block
      .lines()
      .find_map(|line| line.strip_prefix(key)?.strip_prefix(':'))
      .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
  }
}
