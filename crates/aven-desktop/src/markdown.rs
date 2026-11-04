//! Markdown as styled text, reading the same dialect as the terminal app.
use std::ops::Range;

use linkify::{LinkFinder, LinkKind};
use pulldown_cmark::{Alignment, Event, Options, Parser, Tag, TagEnd, TextMergeWithOffset};

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
    /// A list bullet, drawn in place of its glyph: its nesting depth.
    pub bullet: Option<usize>,
    /// A keyboard key, set like code on a key cap.
    pub key: bool,
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

/// The marker column of one logical line in a list item. Continuations have an
/// empty marker range but keep the item's nesting depth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListLine {
    pub line: usize,
    pub depth: usize,
    pub marker: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// How a run of lines is laid out differently from running text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockKind {
    Heading(u8),
    /// A fenced or indented code block, one source line per line.
    Code,
    /// One row per line, with cells separated by tabs. The first row is the
    /// header.
    Table(Vec<Align>),
    /// A thematic break, drawn as a line across the text.
    Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub lines: Range<usize>,
    pub kind: BlockKind,
}

/// Lines joined by newlines, with the styles laid over them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub text: String,
    pub spans: Vec<Span>,
    /// Lines that follow the next one with no paragraph gap.
    pub tight: Vec<usize>,
    pub quotes: Vec<Quote>,
    pub lists: Vec<ListLine>,
    pub blocks: Vec<Block>,
}

/// Bullets by nesting depth, cycling for deeper lists.
const BULLETS: [&str; 3] = ["•  ", "◦  ", "▪  "];
/// Sits either side of inline code and keyboard keys, outside their chips,
/// so a chip doesn't crowd the words next to it.
const CHIP_GAP: &str = "\u{2009}";
/// Labels an image, which the text shows as a link to its source.
const IMAGE_LABEL: &str = "Image: ";

#[derive(Default)]
struct Writer {
    out: Rendered,
    style: Style,
    /// The quotes around the next line, as indices into `out.quotes`.
    open_quotes: Vec<usize>,
    /// How many of the open quotes are block quotes, which dim their text.
    dim_quotes: usize,
    lists: Vec<Option<u64>>,
    /// Whether each list, in source order, is loose: its items hold paragraphs.
    loose: Vec<bool>,
    /// How many lists have started.
    list_count: usize,
    /// Whether each open list is loose.
    open_loose: Vec<bool>,
    items: Vec<usize>,
    /// A marker waiting for the item's first text.
    pending_marker: Option<String>,
    in_code_block: bool,
    in_link: bool,
    /// The block being written, with the line it started on.
    block: Option<(usize, BlockKind)>,
    /// The next table cell is the first in its row.
    first_cell: bool,
    /// Where an image's text starts, to show its source when it has no alt text.
    image: Option<usize>,
    /// Raw HTML of the block being read.
    html: Option<String>,
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
        let start = self.out.text.len();
        let mut marker_start = start;
        if let Some(marker) = self.pending_marker.take() {
            marker_start += marker.len() - marker.trim_start().len();
            let bullet = BULLETS
                .iter()
                .position(|bullet| marker.ends_with(bullet))
                .filter(|_| self.items.last().is_some());
            match bullet {
                Some(depth) => {
                    let (lead, mark) = marker.split_at(marker.len() - BULLETS[depth].len());
                    let glyph = mark.trim_end();
                    self.push(lead, &Style::default());
                    let style = Style {
                        bullet: Some(depth),
                        ..Style::default()
                    };
                    self.push(glyph, &style);
                    self.push(&mark[glyph.len()..], &Style::default());
                }
                None => self.push(&marker, &Style::default()),
            }
        }
        if let Some(&depth) = self.items.last() {
            self.out.lists.push(ListLine {
                line: self.line_count(),
                depth,
                marker: marker_start..self.out.text.len(),
            });
        }
    }

    fn open_block(&mut self, kind: BlockKind) {
        self.block = Some((self.line_count(), kind));
    }

    fn close_block(&mut self) {
        if let Some((start, kind)) = self.block.take() {
            let lines = start..self.line_count() + 1;
            self.out.blocks.push(Block { lines, kind });
        }
    }

    fn text(&mut self, text: &str) {
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
        let style = self.style.clone();
        if self.in_link {
            self.push(text, &style);
            return;
        }
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url, LinkKind::Email]);
        finder.url_must_have_scheme(false);
        let mut at = 0;
        for link in finder.links(text) {
            let found = link.as_str();
            let target = match link.kind() {
                LinkKind::Email => format!("mailto:{found}"),
                _ if found.contains("://") => found.to_string(),
                // Like GitHub, a bare domain links only when it starts with www.
                _ if found.starts_with("www.") => format!("https://{found}"),
                _ => continue,
            };
            self.push(&text[at..link.start()], &style);
            let linked = Style {
                link: Some(target),
                ..style.clone()
            };
            self.push(found, &linked);
            at = link.end();
        }
        self.push(&text[at..], &style);
    }

    fn inline_code(&mut self, text: &str) {
        self.start_text();
        let outside = self.style.clone();
        let style = Style {
            code: true,
            ..outside.clone()
        };
        self.push(CHIP_GAP, &outside);
        self.push(text, &style);
        self.push(CHIP_GAP, &outside);
    }

    /// Applies a formatting tag, returning false for HTML it doesn't know.
    fn html_tag(&mut self, html: &str) -> bool {
        let html = html.trim();
        if html.starts_with("<!--") {
            return true;
        }
        let Some(inner) = html.strip_prefix('<').and_then(|t| t.strip_suffix('>')) else {
            return false;
        };
        let (closing, inner) = match inner.strip_prefix('/') {
            Some(inner) => (true, inner),
            None => (false, inner),
        };
        let name = inner
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let on = !closing;
        match name.as_str() {
            "b" | "strong" | "summary" => self.style.bold = on,
            "i" | "em" => self.style.italic = on,
            "s" | "del" | "strike" => self.style.strike = on,
            "code" | "samp" => self.style.code = on,
            // A key cap holds a thin space either side of its label, and
            // keeps another outside, so caps in a row like Cmd+K stay apart.
            "kbd" => {
                self.style.code = false;
                let style = self.style.clone();
                self.push(CHIP_GAP, &style);
                self.style.key = !self.style.key;
                let style = self.style.clone();
                self.push(CHIP_GAP, &style);
                self.style.code = on;
            }
            // The body shows open, set apart like a quote.
            "details" if on => self.open_quote(),
            "details" => self.close_quote(),
            "br" => {
                self.next_line(true);
                self.start_text();
            }
            "p" | "div" | "span" | "u" | "ins" | "mark" | "sub" | "sup" | "small" | "picture"
            | "source" => {}
            _ => return false,
        }
        true
    }

    fn inline_html(&mut self, html: &str) {
        if !self.html_tag(html) {
            self.text(html);
        }
    }

    /// Writes an HTML block's text with its known tags applied, one line per
    /// source line, leaving out lines that held only tags.
    fn finish_html(&mut self) {
        let Some(html) = self.html.take() else {
            return;
        };
        let mut first = true;
        for line in html.lines() {
            let mut parts = Vec::new();
            let mut rest = line;
            while let Some(open) = rest.find('<') {
                let Some(close) = rest[open..].find('>') else {
                    break;
                };
                parts.push((false, &rest[..open]));
                parts.push((true, &rest[open..open + close + 1]));
                rest = &rest[open + close + 1..];
            }
            parts.push((false, rest));
            let shows = parts.iter().any(|(tag, part)| {
                if *tag {
                    !known_tag(part)
                } else {
                    !part.trim().is_empty()
                }
            });
            if !shows {
                for (_, part) in parts {
                    self.html_tag(part);
                }
                continue;
            }
            self.next_line(!first);
            first = false;
            for (tag, part) in parts {
                if tag {
                    self.inline_html(part);
                } else {
                    let part = if first_on_line(&self.out.text) {
                        part.trim_start()
                    } else {
                        part
                    };
                    if !part.is_empty() {
                        self.text(part);
                    }
                }
            }
        }
        // Tags don't carry styles past their block.
        self.style.bold = false;
        self.style.italic = false;
        self.style.strike = false;
        self.style.code = false;
        self.style.key = false;
    }

    fn open_quote(&mut self) {
        self.open_quotes.push(self.out.quotes.len());
        self.out.quotes.push(Quote {
            lines: 0..0,
            depth: self.open_quotes.len() - 1,
        });
    }

    fn close_quote(&mut self) {
        self.open_quotes.pop();
    }

    fn start(&mut self, tag: Tag<'_>) {
        let after_block = !matches!(tag, Tag::List(_)) && std::mem::take(&mut self.after_block);
        match tag {
            Tag::Paragraph if self.pending_marker.is_none() => self.next_line(false),
            Tag::Heading { level, .. } => {
                self.next_line(false);
                self.open_block(BlockKind::Heading(level as u8));
                self.style.bold = true;
            }
            Tag::HtmlBlock => self.html = Some(String::new()),
            Tag::CodeBlock(_) => {
                self.next_line(false);
                self.open_block(BlockKind::Code);
                self.in_code_block = true;
            }
            Tag::List(first) => {
                self.lists.push(first);
                let loose = self.loose.get(self.list_count).copied().unwrap_or(false);
                self.list_count += 1;
                self.open_loose.push(loose);
                self.list_start = true;
            }
            Tag::Item => {
                let depth = self.lists.len().saturating_sub(1);
                self.items.push(depth);
                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => BULLETS[depth % BULLETS.len()].to_string(),
                };
                let first_in_list = self.fresh || self.out.text.is_empty();
                let loose = self.open_loose.last().copied().unwrap_or(false);
                let hugs = !loose && (!std::mem::take(&mut self.list_start) || !after_block);
                self.next_line(!first_in_list && hugs);
                self.pending_marker = Some(format!("{}{marker}", "   ".repeat(depth)));
            }
            Tag::BlockQuote(_) => {
                self.open_quote();
                self.dim_quotes += 1;
                self.style.dim = true;
            }
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => {
                self.in_link = true;
                self.style.link = Some(dest_url.to_string());
            }
            Tag::Image { dest_url, .. } => {
                self.start_text();
                let label = Style {
                    dim: true,
                    ..self.style.clone()
                };
                self.push(IMAGE_LABEL, &label);
                self.in_link = true;
                self.style.link = Some(dest_url.to_string());
                self.image = Some(self.out.text.len());
            }
            Tag::Table(aligns) => {
                let aligns = aligns
                    .into_iter()
                    .map(|align| match align {
                        Alignment::Center => Align::Center,
                        Alignment::Right => Align::Right,
                        Alignment::Left | Alignment::None => Align::Left,
                    })
                    .collect();
                self.next_line(false);
                self.open_block(BlockKind::Table(aligns));
            }
            Tag::TableHead => {
                self.first_cell = true;
                self.style.bold = true;
            }
            Tag::TableRow => {
                self.next_line(true);
                self.first_cell = true;
            }
            Tag::TableCell => {
                self.start_text();
                if !std::mem::take(&mut self.first_cell) {
                    self.push("\t", &Style::default());
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
        let lead = BULLETS
            .iter()
            .find_map(|bullet| marker.strip_suffix(bullet))
            .unwrap_or(&marker);
        self.pending_marker = Some(lead.to_string());
        self.start_text();
        let check = Style {
            check: Some(checked),
            ..Style::default()
        };
        self.push(if checked { "☑" } else { "☐" }, &check);
        self.push("  ", &Style::default());
        if let Some(line) = self.out.lists.last_mut() {
            line.marker.end = self.out.text.len();
        }
    }

    fn end(&mut self, tag: TagEnd) {
        if matches!(
            tag,
            TagEnd::Heading(_) | TagEnd::CodeBlock | TagEnd::Table | TagEnd::HtmlBlock
        ) {
            self.after_block = true;
        }
        match tag {
            TagEnd::Heading(_) => {
                self.style.bold = false;
                self.close_block();
            }
            TagEnd::CodeBlock => {
                self.in_code_block = false;
                self.start_text();
                self.close_block();
            }
            TagEnd::HtmlBlock => self.finish_html(),
            TagEnd::Item => {
                self.items.pop();
            }
            TagEnd::List(_) => {
                self.lists.pop();
                self.open_loose.pop();
                // Two lists in a row stay apart, like any other blocks.
                self.after_block |= self.lists.is_empty();
            }
            TagEnd::BlockQuote(_) => {
                self.close_quote();
                self.dim_quotes -= 1;
                self.style.dim = self.dim_quotes > 0;
            }
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => {
                self.in_link = false;
                self.style.link = None;
            }
            TagEnd::Image => {
                if self.image.take() == Some(self.out.text.len())
                    && let Some(url) = self.style.link.clone()
                {
                    let style = self.style.clone();
                    self.push(&url, &style);
                }
                self.in_link = false;
                self.style.link = None;
            }
            TagEnd::TableHead => self.style.bold = false,
            TagEnd::Table => self.close_block(),
            _ => {}
        }
    }
}

fn known_tag(tag: &str) -> bool {
    Writer::default().html_tag(tag)
}

fn first_on_line(text: &str) -> bool {
    text.rsplit('\n')
        .next()
        .is_none_or(|line| line.trim().is_empty())
}

/// Whether each list, in source order, is loose: its items hold paragraphs.
fn loose_lists<'a>(events: impl Iterator<Item = &'a Event<'a>>) -> Vec<bool> {
    let mut loose = Vec::new();
    // The open tags, with the list each open list stands for.
    let mut open: Vec<Option<usize>> = Vec::new();
    let mut in_item = Vec::new();
    for event in events {
        match event {
            Event::Start(Tag::List(_)) => {
                open.push(Some(loose.len()));
                in_item.push(false);
                loose.push(false);
            }
            Event::Start(tag) => {
                if matches!(tag, Tag::Paragraph)
                    && in_item.last() == Some(&true)
                    && let Some(&Some(list)) = open.iter().rev().find(|list| list.is_some())
                {
                    loose[list] = true;
                }
                open.push(None);
                in_item.push(matches!(tag, Tag::Item));
            }
            Event::End(_) => {
                open.pop();
                in_item.pop();
            }
            _ => {}
        }
    }
    loose
}

/// The source line holding `offset`.
fn line_of(source: &str, offset: usize) -> usize {
    source[..offset].matches('\n').count()
}

pub fn render(source: &str) -> Rendered {
    let options =
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES | Options::ENABLE_TASKLISTS;
    let events: Vec<_> =
        TextMergeWithOffset::new(Parser::new_ext(source, options).into_offset_iter()).collect();
    let mut writer = Writer {
        fresh: true,
        loose: loose_lists(events.iter().map(|(event, _)| event)),
        ..Writer::default()
    };
    let mut last_end = 0;
    for (event, range) in events {
        let end = range.end;
        match event {
            Event::Start(tag) => {
                // A blank line keeps a list apart from the paragraph above it.
                if matches!(tag, Tag::List(_)) && writer.lists.is_empty() {
                    let above = source[..last_end].trim_end_matches(['\n', '\r']).len();
                    if line_of(source, range.start) > line_of(source, above) + 1 {
                        writer.after_block = true;
                    }
                }
                writer.start(tag);
            }
            Event::End(tag) => writer.end(tag),
            Event::Text(text) => writer.text(&text),
            Event::Code(text) => writer.inline_code(&text),
            Event::TaskListMarker(checked) => writer.task_marker(checked),
            Event::Html(html) => match &mut writer.html {
                Some(block) => block.push_str(&html),
                None => writer.inline_html(&html),
            },
            Event::InlineHtml(html) => writer.inline_html(&html),
            Event::SoftBreak | Event::HardBreak => {
                writer.next_line(true);
                writer.start_text();
            }
            Event::Rule => {
                writer.next_line(false);
                writer.start_text();
                writer.push("───", &Style::default());
                let line = writer.line_count();
                writer.out.blocks.push(Block {
                    lines: line..line + 1,
                    kind: BlockKind::Rule,
                });
                writer.after_block = true;
            }
            _ => {}
        }
        last_end = end;
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
        assert_eq!(out.text, "Some bold, soft, gone and \u{2009}code\u{2009}.");
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
            "Title\nFirst\nline\n•  one\n•  two\n   ◦  deep\n1. a\n2. b\nsaid\n───\nlet a;\nlet b;"
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
            "Scope\n•  a\n•  b\nIntro\n•  c\n   ◦  d\nx\n•  e\n───\n•  f\nt\n•  g\nh\n•  i"
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
            "Before\nOne bold\nTwo\n•  a\n•  b\nInner\nLast \u{2009}code\u{2009}\nAfter"
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
    fn tables_keep_their_cells_styles_and_alignment() {
        let out = render("| a | **bb** | n |\n| - | :-: | -: |\n| [c](https://c.test) | `d` | 1 |");
        assert_eq!(out.text, "a\tbb\tn\nc\t\u{2009}d\u{2009}\t1");
        assert_eq!(
            out.blocks,
            [Block {
                lines: 0..2,
                kind: BlockKind::Table(vec![Align::Left, Align::Center, Align::Right]),
            }]
        );
        assert!(styled(&out, "a").bold);
        assert_eq!(styled(&out, "c").link.as_deref(), Some("https://c.test"));
        assert!(styled(&out, "d").code);
        assert_eq!(out.tight, [0]);
    }

    #[test]
    fn headings_code_and_rules_are_blocks() {
        let out = render("# One\n\n### Three\n\n```\na\n  b\n```\n\n---\n\nend");
        assert_eq!(out.text, "One\nThree\na\n  b\n───\nend");
        assert_eq!(
            out.blocks,
            [
                Block {
                    lines: 0..1,
                    kind: BlockKind::Heading(1)
                },
                Block {
                    lines: 1..2,
                    kind: BlockKind::Heading(3)
                },
                Block {
                    lines: 2..4,
                    kind: BlockKind::Code
                },
                Block {
                    lines: 4..5,
                    kind: BlockKind::Rule
                },
            ]
        );
    }

    #[test]
    fn known_html_tags_style_their_text_and_unknown_ones_stay() {
        let out = render(
            "Press <kbd>K</kbd> for <b>bold</b>, keep Vec<T> as is.<!-- note -->\n\n<details>\n<summary>More</summary>\n\nInside.\n\n</details>\n\n<custom>x</custom>",
        );
        assert_eq!(
            out.text,
            "Press \u{2009}\u{2009}K\u{2009}\u{2009} for bold, keep Vec<T> as is.\nMore\nInside.\n<custom>x</custom>"
        );
        assert!(styled(&out, "K\u{2009}\u{2009} ").key);
        assert!(styled(&out, "bold").bold);
        assert!(styled(&out, "More").bold);
        assert!(!styled(&out, "Inside").bold);
    }

    #[test]
    fn images_link_to_their_source() {
        let out = render("![logo](https://a.test/x.png) and ![](https://b.test/y.png)");
        assert_eq!(out.text, "Image: logo and Image: https://b.test/y.png");
        assert!(styled(&out, "Image").dim && styled(&out, "Image").link.is_none());
        assert_eq!(
            styled(&out, "logo").link.as_deref(),
            Some("https://a.test/x.png")
        );
        assert_eq!(
            styled(&out, "https://b").link.as_deref(),
            Some("https://b.test/y.png")
        );
    }

    #[test]
    fn bare_www_and_email_link_but_bare_file_names_do_not() {
        let out = render("See www.a.test, mail me@b.test, edit main.rs.");
        assert_eq!(
            styled(&out, "www").link.as_deref(),
            Some("https://www.a.test")
        );
        assert_eq!(
            styled(&out, "me@").link.as_deref(),
            Some("mailto:me@b.test")
        );
        assert_eq!(styled(&out, "main.rs").link, None);
    }

    #[test]
    fn separate_lists_keep_a_gap_between_them() {
        let out = render("- a\n- b\n\n* c\n\n1. d");
        assert_eq!(out.text, "•  a\n•  b\n•  c\n1. d");
        assert_eq!(out.tight, [0]);
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
        assert_eq!(styled(&out, "•").bullet, Some(0));
        assert_eq!(styled(&out, "  plain"), Style::default());
        // The numbered list is a list of its own, apart from the one above.
        assert_eq!(out.tight, [0, 1, 2, 3]);
    }

    #[test]
    fn task_lists_keep_their_gaps_and_quotes() {
        let out = render("## Criteria\n\n- [ ] a\n\n- [x] b\n\n> - [ ] c\n\n[ ] plain\n- d [ ] e");
        assert_eq!(
            out.text,
            "Criteria\n☐  a\n☑  b\n☐  c\n[ ] plain\n•  d [ ] e"
        );
        // Blank lines between items make the list loose, so its items keep apart.
        assert_eq!(out.tight, [4]);
        assert_eq!(out.quotes[0].lines, 3..4);
        assert_eq!(
            out.spans.iter().filter(|s| s.style.check.is_some()).count(),
            3
        );
    }

    #[test]
    fn list_columns_distinguish_markers_from_text_and_keep_continuations() {
        let out = render(
            "- **first**\n  continued\n  - nested\n\n  another paragraph\n\n10. ten\n11. eleven\n\n- [ ] open\n- [x] done\n  - [x] nested check\n    continued check\n\n1. [ ] numbered check\n\nOutside",
        );
        let markers: Vec<_> = out
            .lists
            .iter()
            .map(|item| (item.depth, &out.text[item.marker.clone()]))
            .collect();
        assert_eq!(
            markers,
            [
                (0, "•  "),
                (0, ""),
                (1, "◦  "),
                (0, ""),
                (0, "10. "),
                (0, "11. "),
                (0, "☐  "),
                (0, "☑  "),
                (1, "☑  "),
                (1, ""),
                (0, "1. ☐  "),
            ]
        );
        for item in &out.lists {
            let line_start = out
                .text
                .split_inclusive('\n')
                .take(item.line)
                .map(str::len)
                .sum::<usize>();
            assert!(item.marker.start >= line_start);
            assert!(!out.text[item.marker.clone()].contains('\n'));
        }
        assert_eq!(
            out.lists.last().unwrap().line + 1,
            out.text.lines().count() - 1
        );
        assert!(styled(&out, "first").bold);
    }

    #[test]
    fn list_metadata_does_not_treat_literal_markers_as_lists() {
        let out = render(
            "```\n• literal\n1. literal\n```\n\n> - [x] [linked](https://example.com)\n>   continued\n\nOutside",
        );
        assert_eq!(out.lists.len(), 2);
        assert_eq!(out.lists[0].line, 2);
        assert_eq!(&out.text[out.lists[0].marker.clone()], "☑  ");
        assert!(out.lists[1].marker.is_empty());
        assert_eq!(out.quotes[0].lines, 2..4);
        assert_eq!(
            styled(&out, "linked").link.as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn a_blank_line_keeps_a_list_apart_from_the_paragraph_above() {
        let out = render("Close:\n- a\n\nApart:\n\n- b\n\n> Quoted:\n>\n> - c");
        assert_eq!(out.text, "Close:\n•  a\nApart:\n•  b\nQuoted:\n•  c");
        assert_eq!(out.tight, [0]);
    }

    #[test]
    fn loose_items_keep_paragraph_gaps_and_tight_ones_hold_together() {
        let out = render("- a\n\n- b\n\n  more\n- c\n\nText\n- d\n- e");
        assert_eq!(out.text, "•  a\n•  b\nmore\n•  c\nText\n•  d\n•  e");
        assert_eq!(out.tight, [4, 5]);
    }

    #[test]
    fn details_sit_apart_like_a_quote_and_keys_are_marked() {
        let out = render(
            "<details>\n<summary>More</summary>\n\nInside <kbd>K</kbd>.\n\n</details>\n\nAfter",
        );
        assert_eq!(
            out.text,
            "More\nInside \u{2009}\u{2009}K\u{2009}\u{2009}.\nAfter"
        );
        assert_eq!(out.quotes[0].lines, 0..2);
        assert!(!styled(&out, "Inside").dim);
        let key = styled(&out, "K\u{2009}\u{2009}.");
        assert!(key.key && key.code);
    }

    #[test]
    fn empty_text_renders_nothing() {
        assert_eq!(render(""), Rendered::default());
    }
}
