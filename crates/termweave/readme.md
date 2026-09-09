# termweave

Markdown rendered as styled, width-aware terminal lines. Use the same document in a plain
terminal, an ANSI stream, or a Ratatui buffer.

The core has no terminal backend dependency. The optional `ratatui` feature adds conversions
for Ratatui 0.30's text primitives.

```toml
[dependencies]
termweave = "0.1"
# for a Ratatui application:
# termweave = { version = "0.1", features = ["ratatui"] }
```

## Render

```rust
use termweave::{Line, Options, render};

let options = Options {
  width: 72,
  prefix: Line::plain("<reader> "),
  ..Options::default()
};
let document = render("# Result\n\n**Ready** with `code`.", &options);
println!("{}", document.ansi());
// document.plain() produces the same text without styling.
// document.ratatui() produces owned Text when the ratatui feature is enabled.
```

`width` is measured in terminal cells and includes all prefixes and indentation. The prefix
appears once, and later rows align under the message. Nested lists, quotes, and code blocks
keep their own hanging indentation. Wide characters, combining marks, and emoji sequences
wrap as graphemes. A grapheme crossing style boundaries uses the style of its first character.
A width of zero returns no rows. A glyph wider than the available row is
replaced with `�`; a prefix that leaves no room for text is shortened.

`wrap` accepts already styled `Line` values when text should not be interpreted as Markdown.
`Theme` controls text, headings, code, links, and structural markers using terminal palette
indices or RGB colors. Styles include bold, dim, italic, underline, strikethrough, and reverse.

## Stream

```rust
use termweave::{Options, Stream};

let options = Options::default();
let mut stream = Stream::default();
stream.push("**part");
let pending = stream.snapshot(&options);
assert_eq!(pending.plain(), "**part");
stream.push("ial**");
let completed = stream.finish(&options);
assert_eq!(completed.plain(), "partial");
```

Push valid UTF-8 string chunks and take a snapshot when ready to redraw. Each snapshot
reparses the accumulated source. Later delimiters and reference definitions can change earlier
rows, so **replace the previous snapshot**, rather than appending every snapshot to scrollback.
Unclosed inline delimiters remain readable. An open code fence has no closing border until its
closing fence arrives or `finish` is called. Reusing the stream with different `Options` reflows
its text after a resize.

Coalesce incoming chunks before drawing. Parsing and layout are linear in document size per
snapshot; the stream retains the full source and does not impose a byte limit. Applications own
input limits, scheduling, cursor movement, scrollback, and the decision that a run is complete.

## Markdown and output

Parsing uses pulldown-cmark's CommonMark parser, with tables, task lists, strikethrough, and
footnotes enabled. It renders headings, paragraphs, nested emphasis, escapes and entities,
inline and fenced/indented code, nested lists, quotes, rules, links, images as alt text plus
location, and footnotes. Links retain their destination. Tables use aligned columns when there
is room and labeled records when the terminal is narrow. Soft line breaks become spaces;
explicit hard breaks remain line breaks.

HTML is displayed literally. Images are never fetched. Code is styled as a block without
language-specific syntax highlighting. Terminal control characters are shown visibly and tabs
expand to four spaces. ANSI output emits only SGR styles and resets at row boundaries; it
never emits cursor commands or OSC hyperlinks from document content. The application chooses
`plain()` for redirected output or `NO_COLOR`; the library does not inspect environment variables.

ANSI styling ultimately depends on the terminal's font and supported attributes. Ratatui
conversion supports the styles listed above; blinking and underline colors are not part of the
backend-independent style model.

## Examples and development

From this checkout:

```sh
cargo run -p termweave --example render -- 60 < crates/termweave/tests/sample.md
cargo run -p termweave --example stream
cargo test -p termweave --no-default-features
cargo test -p termweave --all-features
cargo clippy -p termweave --all-targets --all-features -- -D warnings
cargo package -p termweave --allow-dirty
```

The crate is self-contained under `crates/termweave` and packages independently of the host
application. Ainz is its first consumer, adding its IRC nick labels, timestamps, tool activity,
and completion state around the rendered lines.

Rust 1.95 or newer. MIT licensed.
