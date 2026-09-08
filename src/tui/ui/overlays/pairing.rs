use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::pairing::{PairingPresentation, PairingQr};
use crate::tui::overlay::{dialog_area, dialog_inner_area};
use crate::tui::text::{cell_width_ranges, str_cells};
use crate::tui::theme::{BG_ALT, FG, FG_MUTED};

const SCAN_INSTRUCTION: &str = "Scan this code with Aven iOS.";
pub(crate) const NETWORK_REQUIREMENT: &str =
    "The phone must reach this server over your VPN/private network.";
const DIALOG_CHROME_COLUMNS: u16 = 4;
const DIALOG_CHROME_ROWS: u16 = 2;
const QR_GAP_ROWS: u16 = 1;
const FOOTER_ROWS: u16 = 1;
const FALLBACK_WIDTH: u16 = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PairingLayout {
    pub(crate) area: Rect,
    pub(crate) inner: Rect,
    pub(crate) content: Rect,
    pub(crate) qr: Option<Rect>,
    pub(crate) footer: Rect,
}

pub(crate) fn pairing_layout(terminal: Rect, presentation: &PairingPresentation) -> PairingLayout {
    let qr_width = u16::try_from(presentation.qr().width()).unwrap_or(u16::MAX);
    let qr_height = u16::try_from(presentation.qr().rows().len()).unwrap_or(u16::MAX);
    let header_height = qr_header_height(presentation.server_identity(), qr_width);
    let required_width = qr_width.saturating_add(DIALOG_CHROME_COLUMNS);
    let required_height = header_height
        .saturating_add(QR_GAP_ROWS)
        .saturating_add(qr_height)
        .saturating_add(QR_GAP_ROWS)
        .saturating_add(FOOTER_ROWS)
        .saturating_add(DIALOG_CHROME_ROWS);
    let fits = required_width <= terminal.width.saturating_sub(2)
        && required_height <= terminal.height.saturating_sub(2);

    if fits {
        let area = dialog_area(terminal, required_width, required_height);
        let inner = dialog_inner_area(area);
        let content = Rect::new(inner.x, inner.y, inner.width, header_height);
        let qr = Rect::new(
            inner.x,
            content.bottom().saturating_add(QR_GAP_ROWS),
            qr_width,
            qr_height,
        );
        return PairingLayout {
            area,
            inner,
            content,
            qr: Some(qr),
            footer: footer_rect(inner),
        };
    }

    let width = FALLBACK_WIDTH.min(terminal.width.saturating_sub(2));
    let inner_width = width.saturating_sub(DIALOG_CHROME_COLUMNS);
    let body_height = wrapped_height(&fallback_text(), inner_width);
    let height = body_height
        .saturating_add(FOOTER_ROWS)
        .saturating_add(DIALOG_CHROME_ROWS);
    let area = dialog_area(terminal, width, height);
    let inner = dialog_inner_area(area);
    PairingLayout {
        area,
        inner,
        content: Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(FOOTER_ROWS),
        ),
        qr: None,
        footer: footer_rect(inner),
    }
}

pub(in crate::tui::ui) fn render_pairing(frame: &mut Frame, presentation: &PairingPresentation) {
    let layout = pairing_layout(frame.area(), presentation);
    let inner = Dialog::new("Pair mobile device", layout.area.width, layout.area.height)
        .render_block_at(frame, layout.area);
    debug_assert_eq!(inner, layout.inner);

    if let Some(qr) = layout.qr {
        render_qr_header(frame, layout.content, presentation.server_identity());
        render_qr(frame, qr, presentation.qr());
    } else {
        render_text(frame, layout.content, &fallback_text(), FG);
    }
    render_footer(frame, layout.footer);
}

fn qr_header_height(server_identity: &str, width: u16) -> u16 {
    wrapped_height(SCAN_INSTRUCTION, width)
        .saturating_add(wrapped_height(&format!("Server: {server_identity}"), width))
        .saturating_add(wrapped_height(NETWORK_REQUIREMENT, width))
}

fn render_qr_header(frame: &mut Frame, area: Rect, server_identity: &str) {
    let scan_height = wrapped_height(SCAN_INSTRUCTION, area.width);
    render_text(
        frame,
        Rect::new(area.x, area.y, area.width, scan_height.min(area.height)),
        SCAN_INSTRUCTION,
        FG,
    );

    let server = format!("Server: {server_identity}");
    let server_height = wrapped_height(&server, area.width);
    let server_y = area.y.saturating_add(scan_height);
    render_text(
        frame,
        Rect::new(
            area.x,
            server_y,
            area.width,
            server_height.min(area.bottom().saturating_sub(server_y)),
        ),
        &server,
        FG_MUTED,
    );

    let requirement_y = server_y.saturating_add(server_height);
    render_text(
        frame,
        Rect::new(
            area.x,
            requirement_y,
            area.width,
            area.bottom().saturating_sub(requirement_y),
        ),
        NETWORK_REQUIREMENT,
        FG_MUTED,
    );
}

fn render_qr(frame: &mut Frame, area: Rect, qr: &PairingQr) {
    let style = Style::new().fg(Color::Black).bg(Color::White);
    let lines = qr
        .rows()
        .iter()
        .map(|row| Line::from(Span::styled(row.as_str(), style)))
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines).style(style), area);
}

fn render_footer(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(dialog_hint_line(&[("Esc", "close")])).style(Style::new().bg(BG_ALT)),
        area,
    );
}

fn render_text(frame: &mut Frame, area: Rect, text: &str, color: Color) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let lines = wrap_text(text, area.width)
        .into_iter()
        .map(Line::from)
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).style(Style::new().fg(color).bg(BG_ALT)),
        area,
    );
}

fn fallback_text() -> String {
    format!(
        "Terminal too small for QR.\n\nRun `aven sync pair` in a larger terminal.\n\n{NETWORK_REQUIREMENT}"
    )
}

fn footer_rect(inner: Rect) -> Rect {
    Rect::new(
        inner.x,
        inner.bottom().saturating_sub(FOOTER_ROWS),
        inner.width,
        u16::from(inner.height >= FOOTER_ROWS),
    )
}

fn wrapped_height(text: &str, width: u16) -> u16 {
    u16::try_from(wrap_text(text, width).len()).unwrap_or(u16::MAX)
}

fn wrap_text(text: &str, width: u16) -> Vec<String> {
    let width = usize::from(width.max(1));
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        if paragraph.trim().is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        for word in paragraph.split_whitespace() {
            let word_width = str_cells(word);
            let current_width = str_cells(&current);
            if word_width <= width {
                if current.is_empty() {
                    current.push_str(word);
                } else if current_width.saturating_add(1).saturating_add(word_width) <= width {
                    current.push(' ');
                    current.push_str(word);
                } else {
                    lines.push(std::mem::take(&mut current));
                    current.push_str(word);
                }
                continue;
            }

            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let ranges = cell_width_ranges(word, width);
            let range_count = ranges.len();
            for (index, (start, end)) in ranges.into_iter().enumerate() {
                let chunk = &word[start..end];
                if index + 1 == range_count && str_cells(chunk) < width {
                    current.push_str(chunk);
                } else {
                    lines.push(chunk.to_string());
                }
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}
