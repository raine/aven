use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::theme::{ACCENT, BG_PANEL, BLUE, FG_DIM, GREEN};

#[derive(Clone, Debug, Default, PartialEq)]
struct Attrs {
    bold: bool,
    italic: bool,
    dimmed: bool,
    underline: bool,
    strikethrough: bool,
    code: bool,
    quote: bool,
    link: bool,
    heading: bool,
    code_block_lang: Option<String>,
    link_url: bool,
    heading_marker: bool,
    code_fence: bool,
}

#[derive(Clone, Debug)]
struct Run {
    text: String,
    attrs: Attrs,
}

#[derive(Clone, Debug)]
struct LayoutLine {
    runs: Vec<Run>,
}

struct ListContext {
    index: Option<u64>,
    depth: usize,
}

#[derive(Clone, Debug)]
struct Prefix {
    cont_text: String,
    cont_attrs: Attrs,
}

struct TableState {
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
}

impl TableState {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
        }
    }
}

struct MarkdownRenderer {
    max_width: usize,
    lines: Vec<LayoutLine>,
    current_line: Vec<Run>,
    current_width: usize,
    attrs_stack: Vec<Attrs>,
    list_stack: Vec<ListContext>,
    prefix_stack: Vec<Prefix>,
    in_code_block: bool,
    code_block_content: String,
    code_block_lang: String,
    in_list_item_start: bool,
    in_block_quote: bool,
    heading_level: Option<u8>,
    link_url: Option<String>,
    table_state: Option<TableState>,
}

impl MarkdownRenderer {
    fn new(max_width: usize) -> Self {
        Self {
            max_width,
            lines: Vec::new(),
            current_line: Vec::new(),
            current_width: 0,
            attrs_stack: Vec::new(),
            list_stack: Vec::new(),
            prefix_stack: Vec::new(),
            in_code_block: false,
            code_block_content: String::new(),
            code_block_lang: String::new(),
            in_list_item_start: false,
            in_block_quote: false,
            heading_level: None,
            link_url: None,
            table_state: None,
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text(&text),
            Event::Code(code) => self.inline_code(&code),
            Event::SoftBreak => self.soft_break(),
            Event::HardBreak => self.hard_break(),
            Event::Rule => self.rule(),
            Event::Html(html) | Event::InlineHtml(html) => self.text(&html),
            _ => {}
        }
    }

    fn current_attrs(&self) -> Attrs {
        let mut attrs = Attrs::default();
        for a in &self.attrs_stack {
            if a.bold {
                attrs.bold = true;
            }
            if a.italic {
                attrs.italic = true;
            }
            if a.dimmed {
                attrs.dimmed = true;
            }
            if a.underline {
                attrs.underline = true;
            }
            if a.strikethrough {
                attrs.strikethrough = true;
            }
            if a.code {
                attrs.code = true;
            }
            if a.quote {
                attrs.quote = true;
            }
            if a.link {
                attrs.link = true;
            }
            if a.heading {
                attrs.heading = true;
            }
        }
        attrs
    }

    fn push_run(&mut self, text: &str, attrs: Attrs) {
        if text.is_empty() {
            return;
        }
        let width = text.width();
        self.current_line.push(Run {
            text: text.to_string(),
            attrs,
        });
        self.current_width += width;
    }

    fn flush_line(&mut self) {
        if !self.current_line.is_empty() {
            self.lines.push(LayoutLine {
                runs: std::mem::take(&mut self.current_line),
            });
        }
        self.current_width = 0;
    }

    fn flush_code_line(&mut self, lang: &str) {
        if self.current_line.is_empty() {
            self.lines.push(LayoutLine {
                runs: vec![Run {
                    text: String::new(),
                    attrs: Attrs {
                        code_block_lang: Some(lang.to_string()),
                        ..Attrs::default()
                    },
                }],
            });
        } else {
            self.lines.push(LayoutLine {
                runs: std::mem::take(&mut self.current_line),
            });
        }
        self.current_width = 0;
    }

    fn break_line_with_indent(&mut self) {
        self.flush_line();
        self.emit_continuation_prefixes();
    }

    fn emit_continuation_prefixes(&mut self) {
        let prefixes: Vec<Prefix> = self.prefix_stack.clone();
        for prefix in prefixes {
            self.push_run(&prefix.cont_text, prefix.cont_attrs);
        }
    }

    fn ensure_blank_line(&mut self) {
        self.flush_line();
        if self.lines.last().is_some_and(|line| !line.runs.is_empty()) {
            self.lines.push(LayoutLine { runs: vec![] });
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Paragraph => {
                if !self.in_list_item_start
                    && !self.in_block_quote
                    && (!self.lines.is_empty() || !self.current_line.is_empty())
                {
                    self.ensure_blank_line();
                }
                self.in_list_item_start = false;
            }
            Tag::Heading { level, .. } => {
                self.ensure_blank_line();
                let level_num = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.heading_level = Some(level_num);
                let prefix = "#".repeat(level_num as usize) + " ";
                self.push_run(
                    &prefix,
                    Attrs {
                        heading: true,
                        heading_marker: true,
                        ..Attrs::default()
                    },
                );
                self.attrs_stack.push(Attrs {
                    heading: true,
                    ..Attrs::default()
                });
            }
            Tag::CodeBlock(kind) => {
                self.ensure_blank_line();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                let fence = if lang.is_empty() {
                    "```".to_string()
                } else {
                    format!("```{lang}")
                };
                self.push_run(
                    &fence,
                    Attrs {
                        dimmed: true,
                        code_fence: true,
                        ..Attrs::default()
                    },
                );
                self.flush_line();
                self.in_code_block = true;
                self.code_block_content.clear();
                self.code_block_lang = lang;
            }
            Tag::List(start) => {
                if self.prefix_stack.is_empty() {
                    self.ensure_blank_line();
                }
                let depth = self.list_stack.len();
                self.list_stack.push(ListContext {
                    index: start,
                    depth,
                });
            }
            Tag::Item => {
                self.flush_line();
                let indent = self
                    .list_stack
                    .last()
                    .map(|ctx| "  ".repeat(ctx.depth))
                    .unwrap_or_default();
                let (bullet, is_numbered) = if let Some(ctx) = self.list_stack.last_mut() {
                    match &mut ctx.index {
                        None => {
                            let bullet = format!("{indent}- ");
                            self.prefix_stack.push(Prefix {
                                cont_text: " ".repeat(bullet.width()),
                                cont_attrs: Attrs::default(),
                            });
                            (bullet, false)
                        }
                        Some(number) => {
                            let bullet = format!("{indent}{number}. ");
                            self.prefix_stack.push(Prefix {
                                cont_text: " ".repeat(bullet.width()),
                                cont_attrs: Attrs::default(),
                            });
                            *number += 1;
                            (bullet, true)
                        }
                    }
                } else {
                    (String::new(), false)
                };
                self.push_run(
                    &bullet,
                    if is_numbered {
                        Attrs {
                            dimmed: true,
                            ..Attrs::default()
                        }
                    } else {
                        Attrs::default()
                    },
                );
                self.in_list_item_start = true;
            }
            Tag::Emphasis => self.attrs_stack.push(Attrs {
                italic: true,
                ..Attrs::default()
            }),
            Tag::Strong => self.attrs_stack.push(Attrs {
                bold: true,
                ..Attrs::default()
            }),
            Tag::Strikethrough => self.attrs_stack.push(Attrs {
                strikethrough: true,
                ..Attrs::default()
            }),
            Tag::BlockQuote(_) => {
                self.ensure_blank_line();
                let quote_attrs = Attrs {
                    quote: true,
                    ..Attrs::default()
                };
                self.push_run("> ", quote_attrs.clone());
                self.prefix_stack.push(Prefix {
                    cont_text: "> ".to_string(),
                    cont_attrs: quote_attrs.clone(),
                });
                self.attrs_stack.push(quote_attrs);
                self.in_block_quote = true;
            }
            Tag::Link { dest_url, .. } => {
                self.link_url = Some(dest_url.to_string());
                self.attrs_stack.push(Attrs {
                    link: true,
                    underline: true,
                    ..Attrs::default()
                });
            }
            Tag::Table(_) => {
                self.ensure_blank_line();
                self.table_state = Some(TableState::new());
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(state) = &mut self.table_state {
                    state.current_row = Vec::new();
                }
            }
            Tag::TableCell => {
                if let Some(state) = &mut self.table_state {
                    state.current_cell = String::new();
                }
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush_line();
            }
            TagEnd::Heading(_) => {
                self.flush_line();
                self.attrs_stack.pop();
                self.heading_level = None;
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                let code = std::mem::take(&mut self.code_block_content);
                let wrapped = wrap_code_lines(&code, self.max_width);
                let lang = std::mem::take(&mut self.code_block_lang);
                for line in wrapped.lines() {
                    if !line.is_empty() {
                        self.push_run(
                            line,
                            Attrs {
                                code_block_lang: Some(lang.clone()),
                                ..Attrs::default()
                            },
                        );
                    }
                    self.flush_code_line(&lang);
                }
                self.push_run(
                    "```",
                    Attrs {
                        dimmed: true,
                        code_fence: true,
                        ..Attrs::default()
                    },
                );
                self.flush_line();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                self.in_list_item_start = false;
            }
            TagEnd::Item => {
                self.flush_line();
                self.prefix_stack.pop();
                self.in_list_item_start = false;
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.attrs_stack.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.attrs_stack.pop();
                self.prefix_stack.pop();
                self.in_block_quote = false;
            }
            TagEnd::Link => {
                self.attrs_stack.pop();
                if let Some(url) = self.link_url.take() {
                    self.push_wrapped_word(
                        &format!(" ({url})"),
                        Attrs {
                            link: true,
                            underline: true,
                            link_url: true,
                            ..Attrs::default()
                        },
                    );
                }
            }
            TagEnd::Table => {
                if let Some(state) = self.table_state.take() {
                    self.lines
                        .extend(render_table_to_lines(&state.rows, self.max_width));
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(state) = &mut self.table_state {
                    let row = std::mem::take(&mut state.current_row);
                    state.rows.push(row);
                }
            }
            TagEnd::TableCell => {
                if let Some(state) = &mut self.table_state {
                    let cell = std::mem::take(&mut state.current_cell);
                    state.current_row.push(cell);
                }
            }
            _ => {}
        }
    }

    fn text(&mut self, text: &str) {
        let text = expand_tabs(text, self.current_width, 8);

        if let Some(state) = &mut self.table_state {
            state.current_cell.push_str(&text.replace('\n', " "));
            return;
        }

        if self.in_code_block {
            self.code_block_content.push_str(&text);
            return;
        }

        let attrs = self.current_attrs();
        let text = text.replace('\n', " ");

        for word in text.split_inclusive(char::is_whitespace) {
            self.push_wrapped_word(word, attrs.clone());
        }
    }

    fn push_wrapped_word(&mut self, word: &str, attrs: Attrs) {
        let word_width = word.width();
        if self.current_width + word_width <= self.max_width
            || self.current_width == 0 && word_width <= self.max_width
        {
            self.push_run(word, attrs);
            return;
        }
        if self.current_width > 0 {
            self.break_line_with_indent();
        }
        if word_width <= self.remaining_width() {
            self.push_run(word, attrs);
            return;
        }

        let mut current = String::new();
        for ch in word.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if current.width() + ch_width > self.remaining_width() && !current.is_empty() {
                self.push_run(&current, attrs.clone());
                self.break_line_with_indent();
                current.clear();
            }
            current.push(ch);
        }
        if !current.is_empty() {
            self.push_run(&current, attrs);
        }
    }

    fn remaining_width(&self) -> usize {
        self.max_width.saturating_sub(self.current_width).max(1)
    }

    fn inline_code(&mut self, code: &str) {
        if let Some(state) = &mut self.table_state {
            state.current_cell.push_str(code);
            return;
        }

        self.push_wrapped_word(
            code,
            Attrs {
                code: true,
                ..Attrs::default()
            },
        );
    }

    fn soft_break(&mut self) {
        self.break_line_with_indent();
    }

    fn hard_break(&mut self) {
        self.break_line_with_indent();
    }

    fn rule(&mut self) {
        self.ensure_blank_line();
        let rule = "─".repeat(self.max_width.min(40));
        self.push_run(
            &rule,
            Attrs {
                dimmed: true,
                ..Attrs::default()
            },
        );
        self.flush_line();
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush_line();
        while self.lines.last().is_some_and(|line| line.runs.is_empty()) {
            self.lines.pop();
        }
        if self.lines.is_empty() {
            return vec![Line::from("")];
        }
        layout_lines_to_ratatui(self.lines)
    }
}

pub(crate) fn render_markdown(input: &str, max_width: usize) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(input, options);
    let mut renderer = MarkdownRenderer::new(max_width.max(1));
    for event in parser {
        renderer.handle_event(event);
    }
    renderer.finish()
}

fn layout_lines_to_ratatui(lines: Vec<LayoutLine>) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .filter(|line| line.runs.is_empty() || line.runs.iter().any(|run| !run.attrs.code_fence))
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .runs
                .into_iter()
                .filter(|run| !run.attrs.heading_marker && !run.attrs.code_fence)
                .map(|run| Span::styled(run.text, attrs_to_style(&run.attrs)))
                .collect();
            if spans.is_empty() {
                Line::from("")
            } else {
                Line::from(spans)
            }
        })
        .collect()
}

fn attrs_to_style(attrs: &Attrs) -> Style {
    let mut style = Style::new();
    if attrs.bold || attrs.heading {
        style = style.add_modifier(Modifier::BOLD);
    }
    if attrs.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if attrs.strikethrough {
        style = style.add_modifier(Modifier::CROSSED_OUT);
    }
    if attrs.dimmed {
        style = style.fg(FG_DIM);
    }
    if attrs.code || attrs.code_block_lang.is_some() {
        style = style.fg(BLUE).bg(BG_PANEL);
    } else if attrs.quote {
        style = style.fg(GREEN);
    } else if attrs.link {
        style = style.fg(BLUE).add_modifier(Modifier::UNDERLINED);
    } else if attrs.heading {
        style = style.fg(ACCENT).add_modifier(Modifier::BOLD);
    }
    style
}

fn expand_tabs(input: &str, start_col: usize, tab_width: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut col = start_col;
    for ch in input.chars() {
        if ch == '\t' {
            let spaces = tab_width - (col % tab_width);
            out.extend(std::iter::repeat_n(' ', spaces));
            col += spaces;
        } else {
            out.push(ch);
            col += UnicodeWidthChar::width(ch).unwrap_or(0);
        }
    }
    out
}

fn wrap_code_lines(code: &str, max_width: usize) -> String {
    if max_width == 0 {
        return code.to_string();
    }

    let mut result = String::new();
    for line in code.lines() {
        let line_width = line.width();
        if line_width <= max_width {
            result.push_str(line);
            result.push('\n');
        } else {
            let mut current_width = 0;
            for ch in line.chars() {
                let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
                if current_width + ch_width > max_width && current_width > 0 {
                    result.push('\n');
                    current_width = 0;
                }
                result.push(ch);
                current_width += ch_width;
            }
            result.push('\n');
        }
    }
    result
}

fn render_table_to_lines(rows: &[Vec<String>], _max_width: usize) -> Vec<LayoutLine> {
    if rows.is_empty() {
        return vec![];
    }

    let num_cols = rows.iter().map(|row| row.len()).max().unwrap_or(0);
    let mut col_widths = vec![0usize; num_cols];

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < num_cols {
                col_widths[index] = col_widths[index].max(cell.trim().width());
            }
        }
    }

    let h = '─';
    let v = '│';
    let tl = '┌';
    let tr = '┐';
    let bl = '└';
    let br = '┘';
    let lj = '├';
    let rj = '┤';
    let tj = '┬';
    let bj = '┴';
    let cj = '┼';

    let mut lines = Vec::new();
    let border_attrs = Attrs {
        dimmed: true,
        ..Attrs::default()
    };

    let build_line = |left: char, mid: char, right: char| -> String {
        let mut line = String::new();
        line.push(left);
        for (index, &width) in col_widths.iter().enumerate() {
            line.extend(std::iter::repeat_n(h, width + 2));
            if index < col_widths.len() - 1 {
                line.push(mid);
            }
        }
        line.push(right);
        line
    };

    lines.push(LayoutLine {
        runs: vec![Run {
            text: build_line(tl, tj, tr),
            attrs: border_attrs.clone(),
        }],
    });

    for (row_index, row) in rows.iter().enumerate() {
        let mut runs = Vec::new();
        runs.push(Run {
            text: v.to_string(),
            attrs: border_attrs.clone(),
        });
        for (index, width) in col_widths.iter().enumerate() {
            let cell = row.get(index).map(|value| value.trim()).unwrap_or("");
            let cell_width = cell.width();
            let padding = width.saturating_sub(cell_width);
            runs.push(Run {
                text: format!(" {cell} "),
                attrs: Attrs::default(),
            });
            if padding > 0 {
                runs.push(Run {
                    text: " ".repeat(padding),
                    attrs: Attrs::default(),
                });
            }
            runs.push(Run {
                text: v.to_string(),
                attrs: border_attrs.clone(),
            });
        }
        lines.push(LayoutLine { runs });

        if row_index < rows.len() - 1 {
            lines.push(LayoutLine {
                runs: vec![Run {
                    text: build_line(lj, cj, rj),
                    attrs: border_attrs.clone(),
                }],
            });
        }
    }

    lines.push(LayoutLine {
        runs: vec![Run {
            text: build_line(bl, bj, br),
            attrs: border_attrs.clone(),
        }],
    });

    lines
}

#[cfg(test)]
fn render_to_text(input: &str, width: usize) -> String {
    render_markdown(input, width)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_one_blank_line() {
        let lines = render_markdown("", 40);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "");
    }

    #[test]
    fn heading_renders_text_without_marker() {
        let rendered = render_to_text("## Context", 40);
        assert!(rendered.contains("Context"));
        assert!(!rendered.contains("##"));
    }

    #[test]
    fn inline_code_wraps_long_tokens() {
        let rendered = render_to_text("Use `日本語テスト` after edits", 8);
        assert!(rendered.contains("日本語"));
        for line in rendered.lines() {
            assert!(line.width() <= 8, "line too wide: {line:?}");
        }
    }

    #[test]
    fn list_continuation_uses_exact_prefix_width() {
        let input = "- Item 1\n  continuation";
        let rendered = render_to_text(input, 40);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].starts_with("  "));
    }

    #[test]
    fn link_renders_text_with_wrapped_url_suffix() {
        let rendered = render_to_text("[docs](https://example.com/docs/with/a/very/long/path)", 16);
        assert!(rendered.contains("docs"));
        assert!(rendered.contains("https://"));
        for line in rendered.lines() {
            assert!(line.width() <= 16, "line too wide: {line:?}");
        }
    }

    #[test]
    fn table_renders_box_drawing() {
        let input = "| A | B |\n|---|---|\n| 1 | 2 |";
        let rendered = render_to_text(input, 40);
        assert!(rendered.contains('┌'));
        assert!(rendered.contains('│'));
        assert!(rendered.contains(" A "));
        assert!(rendered.contains(" 1 "));
    }

    #[test]
    fn unicode_width_wrapping_respects_display_width() {
        let input = "日本語テスト";
        let rendered = render_to_text(input, 4);
        for line in rendered.lines() {
            assert!(line.width() <= 4, "line too wide: {line:?}");
        }
    }

    #[test]
    fn soft_breaks_preserve_source_line_breaks() {
        let input = "Line one\nLine two";
        let rendered = render_to_text(input, 40);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("Line one"));
        assert!(lines[1].contains("Line two"));
    }

    #[test]
    fn bold_list_item_renders_styled_text() {
        let rendered = render_to_text("- **One** item", 40);
        assert!(rendered.contains("- One item"));
    }
}
