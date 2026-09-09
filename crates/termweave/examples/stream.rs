use termweave::{Line, Options, Stream};

fn main() {
  let options = Options {
    width: 36,
    prefix: Line::plain("<reader> "),
    ..Options::default()
  };
  let mut stream = Stream::default();
  for chunk in [
    "**Streaming",
    " Markdown**\n\n- Keep the ",
    "whole grapheme: 👩🏽‍💻\n",
    "- Wrap beneath the prefix.",
  ] {
    stream.push(chunk);
    println!("snapshot:\n{}\n", stream.snapshot(&options).plain());
  }
  println!("finished:\n{}", stream.finish(&options).plain());
}
