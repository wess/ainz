use pulldown_cmark::{Alignment, Tag};

use crate::{
  Line, Theme, inline,
  layout::{LogicalLine, wrapped},
  parse::Node,
};

pub(crate) fn render(
  nodes: &[Node],
  alignments: &[Alignment],
  theme: &Theme,
  width: usize,
) -> Vec<LogicalLine> {
  let rows: Vec<Vec<Line>> = nodes
    .iter()
    .filter_map(|node| {
      let Node::Element(tag @ (Tag::TableHead | Tag::TableRow), children, _) = node else {
        return None;
      };
      let style = if matches!(tag, Tag::TableHead) {
        theme.text.overlay(theme.heading)
      } else {
        theme.text
      };
      Some(
        children
          .iter()
          .filter_map(|cell| {
            let Node::Element(Tag::TableCell, children, _) = cell else {
              return None;
            };
            let mut line = Line::default();
            for part in inline::lines(children, theme, style) {
              if !line.spans.is_empty() {
                line.push(" ", style);
              }
              line.append(&part);
            }
            Some(line)
          })
          .collect(),
      )
    })
    .collect();
  let columns = alignments.len();
  if columns == 0 {
    return Vec::new();
  }
  let mut widths: Vec<usize> = (0..columns)
    .map(|index| {
      rows
        .iter()
        .filter_map(|row| row.get(index))
        .map(Line::width)
        .max()
        .unwrap_or(1)
        .max(1)
    })
    .collect();
  let budget = width.saturating_sub(columns * 3 + 1);
  if budget < columns * 6 {
    return stacked(&rows, theme);
  }
  let mut order: Vec<_> = widths.iter().copied().enumerate().collect();
  order.sort_by_key(|(_, width)| *width);
  let mut remaining = budget;
  for (position, (index, natural)) in order.into_iter().enumerate() {
    widths[index] = natural.min(remaining / (columns - position));
    remaining -= widths[index];
  }
  let mut output = Vec::new();
  for (index, row) in rows.iter().enumerate() {
    let cells: Vec<Vec<Line>> = (0..columns)
      .map(|column| {
        let cell = row.get(column).cloned().unwrap_or_default();
        wrapped(
          &cell,
          &Line::default(),
          &Line::default(),
          widths[column],
          false,
        )
      })
      .collect();
    let height = cells.iter().map(Vec::len).max().unwrap_or(1);
    for row in 0..height {
      let mut line = Line::styled("│", theme.marker);
      for column in 0..columns {
        let cell = cells[column].get(row).cloned().unwrap_or_default();
        let padding = widths[column].saturating_sub(cell.width());
        let left = match alignments[column] {
          Alignment::Right => padding,
          Alignment::Center => padding / 2,
          _ => 0,
        };
        line.push(" ".repeat(left + 1), theme.text);
        line.append(&cell);
        line.push(" ".repeat(padding - left + 1), theme.text);
        line.push("│", theme.marker);
      }
      output.push(LogicalLine {
        content: line,
        literal: true,
        ..LogicalLine::default()
      });
    }
    if index == 0 {
      let rule = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>()
        .join("┼");
      output.push(LogicalLine::new(Line::styled(
        format!("├{rule}┤"),
        theme.marker,
      )));
    }
  }
  output
}

fn stacked(rows: &[Vec<Line>], theme: &Theme) -> Vec<LogicalLine> {
  let Some(headings) = rows.first() else {
    return Vec::new();
  };
  let mut output = Vec::new();
  for row in rows.iter().skip(1) {
    if !output.is_empty() {
      output.push(LogicalLine::default());
    }
    for (index, cell) in row.iter().enumerate() {
      let mut content = headings.get(index).cloned().unwrap_or_default();
      content.push(": ", theme.marker);
      content.append(cell);
      output.push(LogicalLine::new(content));
    }
  }
  if output.is_empty() {
    output.extend(headings.iter().cloned().map(LogicalLine::new));
  }
  output
}
