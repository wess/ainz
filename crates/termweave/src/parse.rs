use pulldown_cmark::{Event, Options as ParseOptions, Parser, Tag};

use crate::{Line, Options, Theme, inline, layout::LogicalLine, table};

#[derive(Debug)]
pub(crate) enum Node {
  Element(Tag<'static>, Vec<Node>),
  Leaf(Event<'static>),
}

pub(crate) fn markdown(source: &str, options: &Options) -> Vec<LogicalLine> {
  let extensions = ParseOptions::ENABLE_TABLES
    | ParseOptions::ENABLE_TASKLISTS
    | ParseOptions::ENABLE_STRIKETHROUGH
    | ParseOptions::ENABLE_FOOTNOTES;
  let mut root = Vec::new();
  let mut stack: Vec<(Tag<'static>, Vec<Node>)> = Vec::new();
  for event in Parser::new_ext(source, extensions) {
    match event {
      Event::Start(tag) => stack.push((tag.into_static(), Vec::new())),
      Event::End(_) => {
        if let Some((tag, children)) = stack.pop() {
          let node = Node::Element(tag, children);
          if let Some((_, parent)) = stack.last_mut() {
            parent.push(node);
          } else {
            root.push(node);
          }
        }
      }
      event => {
        let node = Node::Leaf(event.into_static());
        if let Some((_, parent)) = stack.last_mut() {
          parent.push(node);
        } else {
          root.push(node);
        }
      }
    }
  }
  let width = options
    .width
    .saturating_sub(options.prefix.width().min(options.width.saturating_sub(2)));
  blocks(&root, &options.theme, width, true)
}

fn block(node: &Node) -> bool {
  matches!(
    node,
    Node::Element(
      Tag::Paragraph
        | Tag::Heading { .. }
        | Tag::BlockQuote(_)
        | Tag::CodeBlock(_)
        | Tag::HtmlBlock
        | Tag::List(_)
        | Tag::FootnoteDefinition(_)
        | Tag::Table(_),
      _
    ) | Node::Leaf(Event::Rule)
  )
}

pub(crate) fn blocks(
  nodes: &[Node],
  theme: &Theme,
  width: usize,
  spaced: bool,
) -> Vec<LogicalLine> {
  let mut output = Vec::new();
  let mut index = 0;
  while index < nodes.len() {
    let mut lines = match &nodes[index] {
      Node::Element(Tag::Paragraph, children) => inline::lines(children, theme, theme.text)
        .into_iter()
        .map(LogicalLine::new)
        .collect(),
      Node::Element(Tag::Heading { .. }, children) => {
        inline::lines(children, theme, theme.text.overlay(theme.heading))
          .into_iter()
          .map(LogicalLine::new)
          .collect()
      }
      Node::Element(Tag::BlockQuote(_), children) => {
        let mut lines = blocks(children, theme, width.saturating_sub(2), true);
        let prefix = Line::styled("│ ", theme.marker);
        for line in &mut lines {
          line.indent(&prefix, &prefix);
        }
        lines
      }
      Node::Element(Tag::CodeBlock(_), children) => code(children, theme),
      Node::Element(Tag::HtmlBlock, children) => inline::lines(children, theme, theme.text)
        .into_iter()
        .map(LogicalLine::new)
        .collect(),
      Node::Element(Tag::List(start), children) => {
        let mut number = *start;
        let mut lines = Vec::new();
        for child in children {
          let Node::Element(Tag::Item, item) = child else {
            continue;
          };
          let marker = match number {
            Some(n) => {
              number = Some(n.saturating_add(1));
              format!("{n}. ")
            }
            None => "- ".into(),
          };
          let prefix = Line::styled(&marker, theme.marker);
          let rest = Line::plain(" ".repeat(prefix.width()));
          let mut item = blocks(item, theme, width.saturating_sub(prefix.width()), false);
          if item.is_empty() {
            item.push(LogicalLine::default());
          }
          for (index, line) in item.iter_mut().enumerate() {
            line.indent(if index == 0 { &prefix } else { &rest }, &rest);
          }
          lines.extend(item);
        }
        lines
      }
      Node::Element(Tag::FootnoteDefinition(label), children) => {
        let prefix = Line::styled(format!("[{label}] "), theme.marker);
        let rest = Line::plain(" ".repeat(prefix.width()));
        let mut lines = blocks(children, theme, width.saturating_sub(prefix.width()), true);
        for (index, line) in lines.iter_mut().enumerate() {
          line.indent(if index == 0 { &prefix } else { &rest }, &rest);
        }
        lines
      }
      Node::Element(Tag::Table(alignments), children) => {
        table::render(children, alignments, theme, width)
      }
      Node::Leaf(Event::Rule) => vec![LogicalLine::new(Line::styled(
        "─".repeat(width.min(40)),
        theme.marker,
      ))],
      _ => {
        let start = index;
        index += 1;
        while index < nodes.len() && !block(&nodes[index]) {
          index += 1;
        }
        let lines = inline::lines(&nodes[start..index], theme, theme.text)
          .into_iter()
          .map(LogicalLine::new)
          .collect();
        index -= 1;
        lines
      }
    };
    if !output.is_empty() && !lines.is_empty() && spaced {
      output.push(LogicalLine::default());
    }
    output.append(&mut lines);
    index += 1;
  }
  output
}

fn code(children: &[Node], theme: &Theme) -> Vec<LogicalLine> {
  let text: String = children
    .iter()
    .filter_map(|node| match node {
      Node::Leaf(Event::Text(text)) => Some(text.as_ref()),
      _ => None,
    })
    .collect();
  // Keep decoration out of code, including wrapped rows selected in the terminal.
  text
    .lines()
    .map(|text| LogicalLine {
      content: Line::styled(text, theme.text.overlay(theme.code)),
      literal: true,
      ..LogicalLine::default()
    })
    .collect()
}
