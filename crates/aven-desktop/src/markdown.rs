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
    /// A task list checkbox, drawn in place of its glyph: whether it is done.
    pub check: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub range: Range<usize>,
    pub style: Style,
}

/// A block quote, as lines of the rendered text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub lines: Range<usize>,
    /// How many quotes this one sits inside.
    pub depth: usize,
}

/// Lines joined by newlines, with the styles laid over them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub spans: Vec<Span>,
    /// Lines that follow the next one with no paragraph gap.
    pub tight: Vec<usize>,
    pub quotes: Vec<Quote>,
}

struct Table {
    rows: Vec<Vec<String>>,
    head: bool,
}

#[derive(Default)]
struct Writer {
    out: Rendered,
    style: Style,
    /// The quotes around the next line, as indices into `out.quotes`.
    open_quotes: Vec<usize>,
    lists: Vec<Option<u64>>,
    /// A marker waiting for the item's first text.
    pending_marker: Option<String>,
    in_code_block: bool,
    in_link: bool,
    table: Option<Table>,
    fresh: bool,
    /// The next item is the first in its list.
    list_start: bool,
    /// The last line ends a block that keeps a gap below it, unlike a
    /// paragraph or list item that a list may follow closely.
    after_block: bool,
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
        if !self.out.text.is_empty() || !self.fresh {
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
        for &quote in &self.open_quotes {
            let line = self.line_count();
            let lines = &mut self.out.quotes[quote].lines;
            if lines.start == lines.end {
                lines.start = line;
            }
            lines.end = line + 1;
        }
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
        let after_block = !matches!(tag, Tag::List(_)) && std::mem::take(&mut self.after_block);
        match tag {
            Tag::Paragraph if self.pending_marker.is_none() => self.next_line(false),
            Tag::Heading { .. } => {
                self.next_line(false);
                self.style.bold = true;
            }
            Tag::HtmlBlock => self.next_line(false),
            Tag::CodeBlock(_) => {
                self.next_line(false);
                self.in_code_block = true;
            }
            Tag::List(first) => {
                self.lists.push(first);
                self.list_start = true;
            }
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
                let hugs = !std::mem::take(&mut self.list_start) || !after_block;
                self.next_line(!first_in_list && hugs);
                self.pending_marker = Some(format!("{}{marker}", "   ".repeat(depth)));
            }
            Tag::BlockQuote(_) => {
                self.open_quotes.push(self.out.quotes.len());
                self.out.quotes.push(Quote {
                    lines: 0..0,
                    depth: self.open_quotes.len() - 1,
                });
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

    /// Swaps the item's bullet for a checkbox, or puts one after its number.
    fn task_marker(&mut self, checked: bool) {
        let Some(marker) = self.pending_marker.take() else {
            return;
        };
        let lead = marker.strip_suffix("•  ").unwrap_or(&marker);
        self.pending_marker = Some(lead.to_string());
        self.start_text();
        let check = Style {
            check: Some(checked),
            ..Style::default()
        };
        self.push(if checked { "☑" } else { "☐" }, &check);
        self.push("  ", &Style::default());
    }

    fn end(&mut self, tag: TagEnd) {
        if matches!(
            tag,
            TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock
        ) {
            self.after_block = true;
        }
        match tag {
            TagEnd::Heading(_) => self.style.bold = false,
            TagEnd::CodeBlock => self.in_code_block = false,
            TagEnd::List(_) => {
                self.lists.pop();
            }
            TagEnd::BlockQuote(_) => {
                self.open_quotes.pop();
                self.style.dim = !self.open_quotes.is_empty();
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => {
                self.in_link = false;
                self.style.link = None;
            }
            TagEnd::Image => self.style.dim = !self.open_quotes.is_empty(),
            TagEnd::Table => self.finish_table(),
            _ => {}
        }
    }
}

pub fn render(source: &str) -> Rendered {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
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
            Event::TaskListMarker(checked) => writer.task_marker(checked),
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
                writer.after_block = true;
            }
            _ => {}
        }
    }
    // A list keeps close to the line before it, but not across a quote's edge.
    let Rendered { tight, quotes, .. } = &mut writer.out;
    tight.retain(|line| {
        !quotes
            .iter()
            .any(|quote| [quote.lines.start, quote.lines.end].contains(&(line + 1)))
    });
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
            "Title\nFirst\nline\n•  one\n•  two\n   •  deep\n1. a\n2. b\nsaid\n────────────────────────\nlet a;\nlet b;"
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
    fn a_list_keeps_close_to_a_paragraph_but_not_to_other_blocks() {
        let out = render(
            "## Scope\n\n- a\n- b\n\nIntro\n- c\n  - d\n\n```\nx\n```\n\n- e\n\n---\n\n- f\n\n| t |\n| - |\n\n- g\n\n<div>h</div>\n\n- i",
        );
        assert_eq!(
            out.text,
            "Scope\n•  a\n•  b\nIntro\n•  c\n   •  d\nx\n•  e\n────────────────────────\n•  f\nt\n•  g\n<div>h</div>\n•  i"
        );
        assert_eq!(out.tight, [1, 3, 4]);
    }

    #[test]
    fn a_contiguous_quote_is_one_block_across_paragraphs_and_lists() {
        let out = render(
            "Before\n\n> One **bold**\n>\n> Two\n> - a\n> - b\n>\n> > Inner\n>\n> Last `code`\n\nAfter",
        );
        assert_eq!(
            out.text,
            "Before\nOne bold\nTwo\n•  a\n•  b\nInner\nLast code\nAfter"
        );
        assert_eq!(
            out.quotes,
            [
                Quote {
                    lines: 1..7,
                    depth: 0
                },
                Quote {
                    lines: 5..6,
                    depth: 1
                }
            ]
        );
        let bold = styled(&out, "bold");
        assert!(bold.bold && bold.dim);
        assert!(styled(&out, "code").code);
        assert!(!styled(&out, "After").dim);
    }

    #[test]
    fn a_wrapping_quote_holds_all_its_lines() {
        let out = render("> ```\n> a\n>\n> b\n> ```\n>\n> one\ntwo");
        assert_eq!(out.text, "a\n\nb\none\ntwo");
        assert_eq!(out.quotes[0].lines, 0..5);
    }

    #[test]
    fn an_empty_first_line_keeps_its_place() {
        let out = render("> ```\n>\n> ```\n\nAfter");
        assert_eq!(out.text, "\nAfter");
        assert_eq!(out.quotes[0].lines, 0..1);
        assert_eq!(render("> ```\n>\n> code\n> ```").text, "\ncode");
    }

    #[test]
    fn separate_quotes_stay_separate_and_lists_keep_clear_of_them() {
        let out = render("> a\n\n> b\n\ntext\n\n> c\n\n- d\n\ntext\n\n> - e");
        assert_eq!(out.text, "a\nb\ntext\nc\n•  d\ntext\n•  e");
        let lines: Vec<_> = out.quotes.iter().map(|q| q.lines.clone()).collect();
        assert_eq!(lines, [0..1, 1..2, 3..4, 6..7]);
        assert!(out.tight.is_empty());
    }

    #[test]
    fn a_quote_inside_a_list_item_includes_the_marker_line() {
        let out = render("- > quoted");
        assert_eq!(out.text, "•  quoted");
        assert_eq!(out.quotes[0].lines, 0..1);
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
    fn task_items_swap_their_bullet_for_a_checkbox() {
        let out =
            render("- [ ] open\n- [x] done\n- plain\n  - [X] deep\n    wraps\n\n1. [ ] first");
        assert_eq!(
            out.text,
            "☐  open\n☑  done\n•  plain\n   ☑  deep\nwraps\n1. ☐  first"
        );
        assert_eq!(styled(&out, "☐").check, Some(false));
        assert_eq!(styled(&out, "☑").check, Some(true));
        assert_eq!(styled(&out, "1. ☐").check, None);
        assert_eq!(styled(&out, "  open"), Style::default());
        assert_eq!(styled(&out, "•"), Style::default());
        assert_eq!(out.tight, [0, 1, 2, 3, 4]);
    }

    #[test]
    fn task_lists_keep_their_gaps_and_quotes() {
        let out = render("## Criteria\n\n- [ ] a\n\n- [x] b\n\n> - [ ] c\n\n[ ] plain\n- d [ ] e");
        assert_eq!(
            out.text,
            "Criteria\n☐  a\n☑  b\n☐  c\n[ ] plain\n•  d [ ] e"
        );
        assert_eq!(out.tight, [1, 4]);
        assert_eq!(out.quotes[0].lines, 3..4);
        assert_eq!(
            out.spans.iter().filter(|s| s.style.check.is_some()).count(),
            3
        );
    }

    #[test]
    fn empty_text_renders_nothing() {
        assert_eq!(render(""), Rendered::default());
    }
}
