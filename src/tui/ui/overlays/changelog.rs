use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Span, Text};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use crate::tui::changelog::changelog_dialog_size;
use crate::tui::markdown::render_markdown_reflowed_without_link_urls;
use crate::tui::overlay::dialog_area;
use crate::tui::theme::{BG_ALT, BLUE, FG, FG_MUTED};

const INSTALLED_BADGE_BG: Color = Color::Rgb(55, 56, 52);

pub(in crate::tui::ui) fn render_changelog(frame: &mut Frame, markdown: &str, scroll: u16) {
    let terminal_size = Size::new(frame.area().width, frame.area().height);
    let (width, height) = changelog_dialog_size(terminal_size);
    let rendered = changelog_lines(markdown, width);
    let content = Dialog::new("Changelog", width, height).render_block(frame);
    let body = changelog_body_area(content);
    let text_area = changelog_text_area(body);
    let hint = Rect {
        y: content.y + content.height.saturating_sub(1),
        height: 1,
        ..content
    };
    let start = clamp_scroll_start(scroll, rendered.len(), body.height as usize);

    frame.render_widget(
        Paragraph::new(Text::from(
            rendered
                .iter()
                .skip(start)
                .take(body.height as usize)
                .cloned()
                .collect::<Vec<_>>(),
        ))
        .style(Style::new().fg(FG).bg(BG_ALT)),
        text_area,
    );
    frame.render_widget(
        Paragraph::new(dialog_hint_line(&[
            ("j/k", "line"),
            ("d/u", "half"),
            ("PgUp/PgDn", "page"),
            ("Esc", "close"),
        ]))
        .style(Style::new().fg(FG).bg(BG_ALT)),
        hint,
    );
    render_vertical_scrollbar(frame, body, rendered.len(), scroll);
}

pub(crate) fn changelog_link_at(
    markdown: &str,
    scroll: u16,
    terminal_size: Size,
    column: u16,
    row: u16,
) -> Option<String> {
    let (width, height) = changelog_dialog_size(terminal_size);
    let outer = dialog_area(
        Rect::new(0, 0, terminal_size.width, terminal_size.height),
        width,
        height,
    );
    let content = Rect::new(
        outer.x.saturating_add(2),
        outer.y.saturating_add(1),
        outer.width.saturating_sub(4),
        outer.height.saturating_sub(2),
    );
    let body = changelog_body_area(content);
    let text_area = changelog_text_area(body);
    changelog_link_at_in_area(markdown, scroll, width, body, text_area, column, row)
}

pub(in crate::tui::ui) fn changelog_link_at_in_area(
    markdown: &str,
    scroll: u16,
    render_width: u16,
    body: Rect,
    text_area: Rect,
    column: u16,
    row: u16,
) -> Option<String> {
    if column < text_area.x
        || column >= text_area.x.saturating_add(text_area.width)
        || row < text_area.y
        || row >= text_area.y.saturating_add(text_area.height)
    {
        return None;
    }

    let lines = changelog_lines(markdown, render_width);
    let start = clamp_scroll_start(scroll, lines.len(), body.height as usize);
    let target_line = start.saturating_add(row.saturating_sub(body.y) as usize);
    let links = changelog_links(markdown);
    let mut link_index = 0;
    let mut rendered_link = String::new();

    for (line_index, line) in lines.iter().enumerate().take(target_line.saturating_add(1)) {
        let mut x = body.x;
        for span in &line.spans {
            let span_width = span.content.width() as u16;
            let linked =
                span.style.add_modifier.contains(Modifier::UNDERLINED) && link_index < links.len();
            if linked {
                rendered_link.push_str(&span.content);
                if line_index == target_line && column >= x && column < x.saturating_add(span_width)
                {
                    return resolve_changelog_url(&links[link_index].url);
                }
                if rendered_link.trim().width() >= links[link_index].label.trim().width() {
                    link_index = link_index.saturating_add(1);
                    rendered_link.clear();
                }
            }
            x = x.saturating_add(span_width);
        }
    }
    None
}

pub(in crate::tui::ui) fn changelog_lines(
    markdown: &str,
    width: u16,
) -> Vec<ratatui::text::Line<'static>> {
    let mut rendered = render_markdown_reflowed_without_link_urls(
        markdown,
        width.saturating_sub(6).max(1) as usize,
    );
    for line in &mut rendered {
        let mut spans = Vec::new();
        for span in std::mem::take(&mut line.spans) {
            if span.content == "installed" {
                spans.extend([
                    Span::styled("", Style::new().fg(INSTALLED_BADGE_BG).bg(BG_ALT)),
                    Span::styled(" ●", Style::new().fg(BLUE).bg(INSTALLED_BADGE_BG)),
                    Span::styled(
                        " installed ",
                        Style::new().fg(FG_MUTED).bg(INSTALLED_BADGE_BG),
                    ),
                    Span::styled("", Style::new().fg(INSTALLED_BADGE_BG).bg(BG_ALT)),
                ]);
            } else {
                spans.push(span);
            }
        }
        line.spans = spans;
    }
    let mut lines = vec![ratatui::text::Line::from("")];
    lines.extend(rendered);
    lines.push(ratatui::text::Line::from(""));
    lines
}

fn changelog_text_area(body: Rect) -> Rect {
    Rect {
        width: body.width.saturating_sub(2),
        ..body
    }
}

fn changelog_body_area(content: Rect) -> Rect {
    Rect {
        height: content.height.saturating_sub(1),
        ..content
    }
}

struct ChangelogLink {
    label: String,
    url: String,
}

fn changelog_links(markdown: &str) -> Vec<ChangelogLink> {
    let mut links = Vec::new();
    let mut current: Option<ChangelogLink> = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                current = Some(ChangelogLink {
                    label: String::new(),
                    url: dest_url.to_string(),
                });
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(link) = &mut current {
                    link.label.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(link) = &mut current {
                    link.label.push(' ');
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = current.take() {
                    links.push(link);
                }
            }
            _ => {}
        }
    }
    links
}

fn resolve_changelog_url(url: &str) -> Option<String> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Some(url.to_string())
    } else if url.starts_with('/') {
        Some(format!("https://aven.raine.dev{url}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_adds_top_padding_and_styles_installed_badge() {
        let lines = changelog_lines("## v1.2.3  `installed`", 80);

        assert!(lines[0].to_string().is_empty());
        assert!(lines[1].to_string().contains(" ● installed "));
        assert!(lines.last().unwrap().to_string().is_empty());
    }

    #[test]
    fn reader_hides_link_destinations_and_reserves_scrollbar_gap() {
        let lines = changelog_lines("- [Read the guide](/guide/)", 80);
        let rendered = lines
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect::<Vec<_>>();

        assert_eq!(rendered, vec!["", "- Read the guide", ""]);
        assert_eq!(changelog_text_area(Rect::new(2, 3, 20, 5)).width, 18);
    }

    #[test]
    fn reader_reflows_source_wrapped_entries_to_dialog_width() {
        let lines = changelog_lines("- alpha beta gamma\n  delta epsilon zeta", 32);
        let rendered = lines
            .iter()
            .map(ratatui::text::Line::to_string)
            .collect::<Vec<_>>();

        assert!(rendered[1].contains("gamma delta"));
        assert_eq!(rendered[2].trim(), "epsilon zeta");
        assert!(
            rendered[1..rendered.len() - 1]
                .iter()
                .all(|line| line.width() <= 26)
        );
    }

    #[test]
    fn rendered_markdown_links_have_mouse_targets() {
        let markdown = "## Unreleased\n\n- [Read the guide](/guide/) for details.";
        let size = Size::new(100, 30);

        assert_eq!(
            changelog_link_at(markdown, 0, size, 10, 6),
            Some("https://aven.raine.dev/guide/".to_string())
        );
        assert_eq!(changelog_link_at(markdown, 0, size, 8, 4), None);
    }

    #[test]
    fn relative_documentation_links_resolve_to_the_canonical_site() {
        assert_eq!(
            resolve_changelog_url("/recurring-tasks/"),
            Some("https://aven.raine.dev/recurring-tasks/".to_string())
        );
        assert_eq!(resolve_changelog_url("mailto:test@example.com"), None);
    }
}
