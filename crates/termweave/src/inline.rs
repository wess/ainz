use pulldown_cmark::{Event, Tag};

use crate::{Line, Style, Theme, parse::Node};

pub(crate) fn lines(nodes: &[Node], theme: &Theme, style: Style) -> Vec<Line> {
  let mut output = vec![Line::default()];
  visit(nodes, theme, style, &mut output);
  if output.len() > 1 && output.last().is_some_and(|line| line.spans.is_empty()) {
    output.pop();
  }
  output
}

fn push(lines: &mut Vec<Line>, text: &str, style: Style) {
  let mut parts = text.split('\n').peekable();
  while let Some(part) = parts.next() {
    if let Some(line) = lines.last_mut() {
      line.push(part, style);
    }
    if parts.peek().is_some() {
      lines.push(Line::default());
    }
  }
}

fn visit(nodes: &[Node], theme: &Theme, style: Style, output: &mut Vec<Line>) {
  for node in nodes {
    match node {
      Node::Element(tag, children, _) => {
        let emphasis = match tag {
          Tag::Strong => Style {
            bold: true,
            ..Style::default()
          },
          Tag::Emphasis => Style {
            italic: true,
            ..Style::default()
          },
          Tag::Strikethrough => Style {
            strike: true,
            ..Style::default()
          },
          Tag::Link { .. } | Tag::Image { .. } => theme.link,
          _ => Style::default(),
        };
        if matches!(tag, Tag::Image { .. }) {
          push(output, "[image: ", theme.marker);
        }
        visit(children, theme, style.overlay(emphasis), output);
        if matches!(tag, Tag::Image { .. }) {
          push(output, "]", theme.marker);
        }
        if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } = tag {
          let label = lines(children, theme, style)
            .iter()
            .map(Line::text)
            .collect::<Vec<_>>()
            .join(" ");
          if label != dest_url.as_ref() && !dest_url.is_empty() {
            push(output, &format!(" ({dest_url})"), theme.marker);
          }
        }
      }
      Node::Leaf(event) => match event {
        Event::Text(text) | Event::Html(text) | Event::InlineHtml(text) => {
          push(output, text, style)
        }
        Event::Code(text) => push(output, text, style.overlay(theme.code)),
        Event::SoftBreak => push(output, " ", style),
        Event::HardBreak => output.push(Line::default()),
        Event::FootnoteReference(label) => push(output, &format!("[{label}]"), theme.link),
        Event::TaskListMarker(done) => {
          push(output, if *done { "[x] " } else { "[ ] " }, theme.marker)
        }
        _ => {}
      },
    }
  }
}
