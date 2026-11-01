//! Markdown as styled text, reading the same dialect as the terminal app.
use std::ops::Range;

use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd, TextMergeStream};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
    pub dim: bool,
    pub link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// Lines joined by newlines, with the styles laid over them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub spans: Vec<Span>,
    /// Lines that follow the next one with no paragraph gap.
    pub tight: Vec<usize>,
}

struct Table {
    rows: Vec<Vec<String>>,
    head: bool,
}

#[derive(Default)]
struct Writer {
    out: Rendered,
    style: Style,
    /// Where nesting puts each new line.
    prefix: Vec<String>,
    lists: Vec<Option<u64>>,
    /// A marker waiting for the item's first text.
    pending_marker: Option<String>,
    in_code_block: bool,
    in_link: bool,
    table: Option<Table>,
    fresh: bool,
}

impl Writer {
    fn line_count(&self) -> usize {
        self.out.text.matches('\n').count()
    }

    fn push(&mut self, text: &str, style: &Style) {
        if text.is_empty() {
            return;
        }
        let start = self.out.text.len();
        self.out.text.push_str(text);
        if *style != Style::default() {
            let range = start..self.out.text.len();
            self.out.spans.push(Span {
                range,
                style: style.clone(),
            });
        }
    }

    /// Starts a line, keeping it close to the last one when `tight`.
    fn next_line(&mut self, tight: bool) {
        if !self.out.text.is_empty() {
            if tight {
                let line = self.line_count();
                self.out.tight.push(line);
            }
            self.out.text.push('\n');
        }
        self.fresh = true;
    }

    fn start_text(&mut self) {
        if !self.fresh {
            return;
        }
        self.fresh = false;
        let prefix = self.prefix.concat();
        self.push(&prefix, &Style::default());
        if let Some(marker) = self.pending_marker.take() {
            self.push(&marker, &Style::default());
        }
    }

    fn text(&mut self, text: &str) {
        if let Some(table) = &mut self.table {
            if let Some(cell) = table.rows.last_mut().and_then(|row| row.last_mut()) {
                cell.push_str(text);
            }
            return;
        }
        if self.in_code_block {
            let mut lines = text.strip_suffix('\n').unwrap_or(text).split('\n');
            let style = Style {
                code: true,
                ..self.style.clone()
            };
            if let Some(first) = lines.next() {
                self.start_text();
                self.push(first, &style);
            }
            for line in lines {
                self.next_line(true);
                self.start_text();
                self.push(line, &style);
            }
            return;
        }
        self.start_text();
        if self.in_link {
            let style = self.style.clone();
            self.push(text, &style);
            return;
        }
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]);
        let mut at = 0;
        let style = self.style.clone();
        for link in finder.links(text) {
            self.push(&text[at..link.start()], &style);
            let linked = Style {
                link: Some(link.as_str().to_string()),
                ..style.clone()
            };
            self.push(link.as_str(), &linked);
            at = link.end();
        }
        self.push(&text[at..], &style);
    }

    fn inline_code(&mut self, text: &str) {
        if let Some(table) = &mut self.table {
            if let Some(cell) = table.rows.last_mut().and_then(|row| row.last_mut()) {
                cell.push_str(text);
            }
            return;
        }
        self.start_text();
        let style = Style {
            code: true,
            ..self.style.clone()
        };
        self.push(text, &style);
    }

    fn finish_table(&mut self) {
        let Some(table) = self.table.take() else {
            return;
        };
        let mut widths = Vec::new();
        for row in &table.rows {
            for (ix, cell) in row.iter().enumerate() {
                let width = cell.chars().count();
                match widths.get_mut(ix) {
                    Some(column) => *column = width.max(*column),
                    None => widths.push(width),
                }
            }
        }
        for (ix, row) in table.rows.iter().enumerate() {
            let cells: Vec<String> = row
                .iter()
                .zip(&widths)
                .map(|(cell, width)| format!("{cell:<width$}"))
                .collect();
            let style = Style {
                code: true,
                bold: table.head && ix == 0,
                ..Style::default()
            };
            self.next_line(ix > 0);
            self.start_text();
            self.push(cells.join("  ").trim_end(), &style);
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph if self.pending_marker.is_none() => self.next_line(false),
            Tag::Heading { .. } => {
                self.next_line(false);
                self.style.bold = true;
            }
            Tag::CodeBlock(_) => {
                self.next_line(false);
                self.in_code_block = true;
            }
            Tag::List(first) => self.lists.push(first),
            Tag::Item => {
                let depth = self.lists.len().saturating_sub(1);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "•  ".to_string(),
                };
                let first_in_list = self.fresh || self.out.text.is_empty();
                self.next_line(!first_in_list);
                self.pending_marker = Some(format!("{}{marker}", "   ".repeat(depth)));
            }
            Tag::BlockQuote(_) => {
                self.prefix.push("▎ ".to_string());
                self.style.dim = true;
            }
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => {
                self.in_link = true;
                self.style.link = Some(dest_url.to_string());
            }
            Tag::Image { .. } => self.style.dim = true,
            Tag::Table(_) => {
                self.table = Some(Table {
                    rows: Vec::new(),
                    head: false,
                });
            }
            Tag::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = true;
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(row) = self.table.as_mut().and_then(|t| t.rows.last_mut()) {
                    row.push(String::new());
                }
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) => self.style.bold = false,
            TagEnd::CodeBlock => self.in_code_block = false,
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.prefix.pop();
                self.style.dim = !self.prefix.is_empty();
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => {
                self.in_link = false;
                self.style.link = None;
            }
            TagEnd::Image => self.style.dim = !self.prefix.is_empty(),
            TagEnd::Table => self.finish_table(),
            _ => {}
        }
    }
}

pub fn render(source: &str) -> Rendered {
    let options = Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES;
    let mut writer = Writer {
        fresh: true,
        ..Writer::default()
    };
    for event in TextMergeStream::new(Parser::new_ext(source, options)) {
        match event {
            Event::Start(tag) => writer.start(tag),
            Event::End(tag) => writer.end(tag),
            Event::Text(text) => writer.text(&text),
            Event::Code(text) => writer.inline_code(&text),
            Event::Html(html) | Event::InlineHtml(html) => writer.text(html.trim_end()),
            Event::SoftBreak | Event::HardBreak => {
                writer.next_line(true);
                writer.start_text();
            }
            Event::Rule => {
                writer.next_line(false);
                writer.start_text();
                let dim = Style {
                    dim: true,
                    ..Style::default()
                };
                writer.push(&"─".repeat(24), &dim);
            }
            _ => {}
        }
    }
    writer.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styled(rendered: &Rendered, part: &str) -> Style {
        let at = rendered.text.find(part).unwrap();
        rendered
            .spans
            .iter()
            .find(|span| span.range.contains(&at))
            .map_or(Style::default(), |span| span.style.clone())
    }

    #[test]
    fn inline_styles_lose_their_marks() {
        let out = render("Some **bold**, *soft*, ~~gone~~ and `code`.");
        assert_eq!(out.text, "Some bold, soft, gone and code.");
        assert!(styled(&out, "bold").bold);
        assert!(styled(&out, "soft").italic);
        assert!(styled(&out, "gone").strike);
        assert!(styled(&out, "code").code);
    }

    #[test]
    fn blocks_keep_the_terminal_apps_shapes() {
        let out = render(
            "# Title\n\nFirst\nline\n\n- one\n- two\n  - deep\n\n1. a\n2. b\n\n> said\n\n---\n\n```rust\nlet a;\nlet b;\n```\n",
        );
        assert_eq!(
            out.text,
            "Title\nFirst\nline\n•  one\n•  two\n   •  deep\n1. a\n2. b\n▎ said\n────────────────────────\nlet a;\nlet b;"
        );
        assert!(styled(&out, "Title").bold);
        assert!(styled(&out, "said").dim);
        assert!(styled(&out, "let b").code);
        // Lists and code hold together, paragraphs do not.
        assert!(out.tight.contains(&1));
        assert!(out.tight.contains(&3));
        assert!(!out.tight.contains(&0));
    }

    #[test]
    fn links_open_their_target_and_bare_urls_link_outside_code() {
        let out = render("[site](https://a.test) and https://b.test/x but `https://c.test`");
        assert_eq!(styled(&out, "site").link.as_deref(), Some("https://a.test"));
        assert_eq!(
            styled(&out, "https://b.test/x").link.as_deref(),
            Some("https://b.test/x")
        );
        assert_eq!(styled(&out, "https://c.test").link, None);
    }

    #[test]
    fn tables_line_up_and_html_and_images_stay_readable() {
        let out = render("| a | bb |\n| - | - |\n| ccc | d |\n\n<b>raw</b>\n\n![alt text](x.png)");
        assert!(out.text.starts_with("a    bb\nccc  d"));
        assert!(styled(&out, "a    bb").bold);
        assert!(out.text.contains("<b>raw</b>"));
        assert!(out.text.ends_with("alt text"));
    }

    #[test]
    fn empty_text_renders_nothing() {
        assert_eq!(render(""), Rendered::default());
    }
}
