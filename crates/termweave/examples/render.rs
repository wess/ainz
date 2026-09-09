use std::io::{self, IsTerminal, Read};

use termweave::{Options, render};

fn main() -> Result<(), Box<dyn std::error::Error>> {
  let width = std::env::args()
    .nth(1)
    .map(|value| value.parse())
    .transpose()?
    .unwrap_or(80);
  let mut source = String::new();
  io::stdin().read_to_string(&mut source)?;
  let document = render(
    &source,
    &Options {
      width,
      ..Options::default()
    },
  );
  if io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none() {
    println!("{}", document.ansi());
  } else {
    println!("{}", document.plain());
  }
  Ok(())
}
