use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::overlay::UpdateOverlayState;
use crate::tui::theme::{ACCENT, BG_ALT, FG, FG_DIM, GREEN, ORANGE, RED};

pub(in crate::tui::ui) fn render_update(frame: &mut Frame, state: &UpdateOverlayState) {
    let lines = update_lines(state);
    let width = frame.area().width.saturating_sub(8).clamp(48, 84);
    let height = (lines.len() as u16)
        .saturating_add(2)
        .min(frame.area().height.saturating_sub(2))
        .max(5);
    let content = Dialog::new(update_title(state), width, height).render_block(frame);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::new().fg(FG).bg(BG_ALT))
            .wrap(Wrap { trim: false }),
        content,
    );
}

fn update_title(state: &UpdateOverlayState) -> &'static str {
    match state {
        UpdateOverlayState::Success { .. } => "Update installed",
        UpdateOverlayState::Failed { .. } => "Update failed",
        UpdateOverlayState::Cancelled => "Update cancelled",
        _ => "Update aven",
    }
}

fn update_lines(state: &UpdateOverlayState) -> Vec<Line<'static>> {
    match state {
        UpdateOverlayState::Checking => vec![
            status_line("●", "Checking GitHub for the latest stable release", ACCENT),
            Line::from(""),
            dialog_hint_line(&[("Esc", "cancel")]),
        ],
        UpdateOverlayState::Progress {
            version,
            phase,
            cancelling,
        } => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("target  ", Style::new().fg(FG_DIM)),
                    Span::styled(format!("v{version}"), Style::new().fg(ACCENT)),
                ]),
                Line::from(""),
                status_line(
                    if *cancelling { "◌" } else { "●" },
                    if *cancelling {
                        "Cancelling update"
                    } else {
                        phase.label()
                    },
                    if *cancelling { ORANGE } else { ACCENT },
                ),
                Line::from(""),
            ];
            lines.push(if phase.cancellable() {
                dialog_hint_line(&[("Esc", "cancel")])
            } else {
                Line::from(Span::styled(
                    "Finishing the installation. Keep aven open.",
                    Style::new().fg(FG_DIM),
                ))
            });
            lines
        }
        UpdateOverlayState::Current { version, cached } => {
            let mut lines = vec![status_line(
                "✓",
                &format!("Aven v{version} is up to date"),
                GREEN,
            )];
            if *cached {
                lines.push(cached_line(true));
            }
            lines.push(Line::from(""));
            lines.push(dialog_hint_line(&[("Esc", "close")]));
            lines
        }
        UpdateOverlayState::Guidance {
            version,
            lines,
            cached,
        } => {
            let mut rendered = vec![status_line(
                "↑",
                &format!("Aven v{version} is available"),
                ACCENT,
            )];
            if *cached {
                rendered.push(cached_line(true));
            }
            rendered.push(Line::from(""));
            rendered.extend(lines.iter().cloned().map(Line::from));
            rendered.push(Line::from(""));
            rendered.push(dialog_hint_line(&[("Esc", "close")]));
            rendered
        }
        UpdateOverlayState::Success { version } => vec![
            status_line("✓", &format!("Installed aven v{version}"), GREEN),
            Line::from(""),
            Line::from("Restart aven to use the installed version."),
            Line::from("Your tasks and current database are unchanged."),
            Line::from(""),
            dialog_hint_line(&[("q", "quit"), ("Esc", "continue")]),
        ],
        UpdateOverlayState::Failed { message } => vec![
            status_line("×", message, RED),
            Line::from(""),
            Line::from(Span::styled(
                "The existing aven executable is unchanged.",
                Style::new().fg(FG_DIM),
            )),
            Line::from(""),
            dialog_hint_line(&[("Enter", "retry"), ("Esc", "close")]),
        ],
        UpdateOverlayState::Cancelled => vec![
            status_line("○", "The update was cancelled", ORANGE),
            Line::from(""),
            Line::from("The existing aven executable is unchanged."),
            Line::from(""),
            dialog_hint_line(&[("Enter", "try again"), ("Esc", "close")]),
        ],
    }
}

fn status_line(icon: &str, message: &str, color: ratatui::style::Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_string(), Style::new().fg(FG)),
    ])
}

fn cached_line(cached: bool) -> Line<'static> {
    if cached {
        Line::from(Span::styled(
            "Showing cached release information because a live check was unavailable.",
            Style::new().fg(FG_DIM),
        ))
    } else {
        Line::from("")
    }
}

#[cfg(test)]
pub(in crate::tui::ui) fn update_lines_for_test(state: &UpdateOverlayState) -> Vec<Line<'static>> {
    update_lines(state)
}
