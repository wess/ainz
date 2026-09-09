use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, Options as ParseOptions, Parser, Tag};

use crate::{Line, Options, Theme, inline, layout::LogicalLine, table};

#[derive(Debug)]
pub(crate) enum Node {
  Element(Tag<'static>, Vec<Node>, Range<usize>),
  Leaf(Event<'static>),
}

pub(crate) fn markdown(source: &str, options: &Options, complete: bool) -> Vec<LogicalLine> {
  let extensions = ParseOptions::ENABLE_TABLES
    | ParseOptions::ENABLE_TASKLISTS
    | ParseOptions::ENABLE_STRIKETHROUGH
    | ParseOptions::ENABLE_FOOTNOTES;
  let mut root = Vec::new();
  let mut stack: Vec<(Tag<'static>, Vec<Node>, usize)> = Vec::new();
  for (event, range) in Parser::new_ext(source, extensions).into_offset_iter() {
    match event {
      Event::Start(tag) => stack.push((tag.into_static(), Vec::new(), range.start)),
      Event::End(_) => {
        if let Some((tag, children, start)) = stack.pop() {
          let node = Node::Element(tag, children, start..range.end);
          if let Some((_, parent, _)) = stack.last_mut() {
            parent.push(node);
          } else {
            root.push(node);
          }
        }
      }
      event => {
        let node = Node::Leaf(event.into_static());
        if let Some((_, parent, _)) = stack.last_mut() {
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
  blocks(&root, &options.theme, source, complete, width, true)
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
      _,
      _
    ) | Node::Leaf(Event::Rule)
  )
}

pub(crate) fn blocks(
  nodes: &[Node],
  theme: &Theme,
  source: &str,
  complete: bool,
  width: usize,
  spaced: bool,
) -> Vec<LogicalLine> {
  let mut output = Vec::new();
  let mut index = 0;
  while index < nodes.len() {
    let mut lines = match &nodes[index] {
      Node::Element(Tag::Paragraph, children, _) => inline::lines(children, theme, theme.text)
        .into_iter()
        .map(LogicalLine::new)
        .collect(),
      Node::Element(Tag::Heading { .. }, children, _) => {
        inline::lines(children, theme, theme.text.overlay(theme.heading))
          .into_iter()
          .map(LogicalLine::new)
          .collect()
      }
      Node::Element(Tag::BlockQuote(_), children, _) => {
        let mut lines = blocks(
          children,
          theme,
          source,
          complete,
          width.saturating_sub(2),
          true,
        );
        let prefix = Line::styled("│ ", theme.marker);
        for line in &mut lines {
          line.indent(&prefix, &prefix);
        }
        lines
      }
      Node::Element(Tag::CodeBlock(kind), children, range) => {
        code(kind, children, theme, &source[range.clone()], complete)
      }
      Node::Element(Tag::HtmlBlock, children, _) => inline::lines(children, theme, theme.text)
        .into_iter()
        .map(LogicalLine::new)
        .collect(),
      Node::Element(Tag::List(start), children, _) => {
        let mut number = *start;
        let mut lines = Vec::new();
        for child in children {
          let Node::Element(Tag::Item, item, _) = child else {
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
          let mut item = blocks(
            item,
            theme,
            source,
            complete,
            width.saturating_sub(prefix.width()),
            false,
          );
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
      Node::Element(Tag::FootnoteDefinition(label), children, _) => {
        let prefix = Line::styled(format!("[{label}] "), theme.marker);
        let rest = Line::plain(" ".repeat(prefix.width()));
        let mut lines = blocks(
          children,
          theme,
          source,
          complete,
          width.saturating_sub(prefix.width()),
          true,
        );
        for (index, line) in lines.iter_mut().enumerate() {
          line.indent(if index == 0 { &prefix } else { &rest }, &rest);
        }
        lines
      }
      Node::Element(Tag::Table(alignments), children, _) => {
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

fn code(
  kind: &CodeBlockKind<'_>,
  children: &[Node],
  theme: &Theme,
  source: &str,
  complete: bool,
) -> Vec<LogicalLine> {
  let language = match kind {
    CodeBlockKind::Fenced(info) => info.as_ref(),
    _ => "",
  };
  let mut lines = vec![LogicalLine::new(Line::styled(
    format!("┌ {language}"),
    theme.marker,
  ))];
  let text: String = children
    .iter()
    .filter_map(|node| match node {
      Node::Leaf(Event::Text(text)) => Some(text.as_ref()),
      _ => None,
    })
    .collect();
  for text in text.lines() {
    lines.push(LogicalLine {
      content: Line::styled(text, theme.text),
      prefix: Line::styled("│ ", theme.marker),
      continuation: Line::styled("│ ", theme.marker),
      literal: true,
    });
  }
  if complete || matches!(kind, CodeBlockKind::Indented) || closed_fence(source) {
    lines.push(LogicalLine::new(Line::styled("└", theme.marker)));
  }
  lines
}

fn closed_fence(source: &str) -> bool {
  let mut lines = source.lines();
  let first = lines.next().unwrap_or_default().trim_start();
  let Some(marker @ ('`' | '~')) = first.chars().next() else {
    return false;
  };
  let count = first.chars().take_while(|ch| *ch == marker).count();
  let Some(last) = lines.next_back() else {
    return false;
  };
  let last = last.trim_start_matches([' ', '\t', '>']);
  let closing = last.chars().take_while(|ch| *ch == marker).count();
  closing >= count && last[closing..].trim().is_empty()
}
