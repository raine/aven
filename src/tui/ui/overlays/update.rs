use ratatui::Frame;
use ratatui::layout::{Alignment, Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Paragraph, Wrap};

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::changelog::{changelog_line_count, changelog_lines, changelog_link_at_in_area};
use crate::tui::overlay::{UpdateActionFocus, UpdateNotesState, UpdateOverlayState, dialog_area};
use crate::tui::theme::Theme;

pub(in crate::tui::ui) fn render_update(
    frame: &mut Frame,
    theme: &Theme,
    state: &UpdateOverlayState,
) {
    let terminal_size = Size::new(frame.area().width, frame.area().height);
    let (width, height) = update_dialog_size(terminal_size);
    let content = Dialog::new(theme, update_title(state), width, height).render_block(frame);
    if let UpdateOverlayState::Available {
        plan,
        notes,
        scroll,
        focus,
        cached: _,
    } = state
    {
        render_available_update(frame, theme, content, plan, notes, *scroll, *focus);
        return;
    }

    frame.render_widget(
        Paragraph::new(Text::from(update_lines(theme, state)))
            .style(Style::new().fg(theme.fg).bg(theme.bg_raised))
            .wrap(Wrap { trim: false }),
        content,
    );
}

pub(crate) fn update_dialog_size(terminal_size: Size) -> (u16, u16) {
    (
        terminal_size.width.saturating_sub(8).clamp(56, 96),
        terminal_size.height.saturating_sub(4).clamp(12, 30),
    )
}

pub(crate) fn update_notes_scroll_cap(notes: &UpdateNotesState, terminal_size: Size) -> u16 {
    let UpdateNotesState::Ready(markdown) = notes else {
        return 0;
    };
    let (width, height) = update_dialog_size(terminal_size);
    let visible_rows = height.saturating_sub(7) as usize;
    changelog_line_count(markdown, width)
        .saturating_sub(visible_rows)
        .try_into()
        .unwrap_or(u16::MAX)
}

pub(crate) fn update_action_at(
    state: &UpdateOverlayState,
    terminal_size: Size,
    column: u16,
    row: u16,
) -> Option<UpdateActionFocus> {
    let (primary_label, later_label) = match state {
        UpdateOverlayState::Available { plan, .. } => {
            let primary = if guidance_has_command(plan) {
                " Copy command "
            } else if plan.guidance().is_some() {
                " Copy instructions "
            } else {
                " Update "
            };
            (primary, " Later ")
        }
        _ => return None,
    };
    let (width, height) = update_dialog_size(terminal_size);
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
    let footer_row = content.y.saturating_add(content.height.saturating_sub(1));
    if row != footer_row {
        return None;
    }
    let primary_width = primary_label.len() as u16;
    let later_width = later_label.len() as u16;
    let primary_start = content
        .x
        .saturating_add(content.width.saturating_sub(primary_width));
    let later_start = primary_start.saturating_sub(later_width.saturating_add(1));
    if column >= primary_start && column < primary_start.saturating_add(primary_width) {
        Some(UpdateActionFocus::Primary)
    } else if column >= later_start && column < later_start.saturating_add(later_width) {
        Some(UpdateActionFocus::Later)
    } else {
        None
    }
}

pub(crate) fn update_link_at(
    state: &UpdateOverlayState,
    terminal_size: Size,
    column: u16,
    row: u16,
) -> Option<String> {
    let UpdateOverlayState::Available {
        notes: UpdateNotesState::Ready(markdown),
        scroll,
        ..
    } = state
    else {
        return None;
    };
    let (width, height) = update_dialog_size(terminal_size);
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
    let notes_area = Rect::new(
        content.x,
        content.y.saturating_add(4),
        content.width,
        content.height.saturating_sub(5),
    );
    let text_area = Rect {
        width: notes_area.width.saturating_sub(2),
        ..notes_area
    };
    changelog_link_at_in_area(
        markdown,
        *scroll,
        content.width,
        notes_area,
        text_area,
        column,
        row,
    )
}

fn render_available_update(
    frame: &mut Frame,
    theme: &Theme,
    content: Rect,
    plan: &crate::update::InstallPlan,
    notes: &UpdateNotesState,
    scroll: u16,
    focus: UpdateActionFocus,
) {
    let version = &plan.release.version;
    let headline = Rect::new(content.x, content.y, content.width, 1);
    let metadata = Rect::new(content.x, content.y.saturating_add(1), content.width, 1);
    let section = Rect::new(content.x, content.y.saturating_add(3), content.width, 1);
    let footer = Rect::new(
        content.x,
        content.y.saturating_add(content.height.saturating_sub(1)),
        content.width,
        1,
    );
    let notes_area = Rect::new(
        content.x,
        content.y.saturating_add(4),
        content.width,
        content.height.saturating_sub(5),
    );
    let notes_text_area = Rect {
        width: notes_area.width.saturating_sub(2),
        ..notes_area
    };

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "↑ ",
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("Aven v{version} is available"),
                Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
        ]))
        .style(Style::new().bg(theme.bg_raised)),
        headline,
    );
    let metadata_text = format!(
        "You have v{} · {}",
        crate::update::CURRENT_VERSION,
        install_label(plan)
    );
    frame.render_widget(
        Paragraph::new(metadata_text).style(theme.dim().bg(theme.bg_raised)),
        metadata,
    );
    if let Some(lines) = plan.guidance() {
        let detail = lines
            .iter()
            .find_map(|line| {
                line.strip_prefix("Run: ")
                    .map(|command| format!("Run: {command}"))
            })
            .unwrap_or_else(|| lines.into_iter().skip(1).collect::<Vec<_>>().join(" "));
        frame.render_widget(
            Paragraph::new(detail).style(theme.muted().bg(theme.bg_raised)),
            Rect::new(content.x, content.y.saturating_add(2), content.width, 1),
        );
    }
    frame.render_widget(
        Paragraph::new("Changelog").style(
            Style::new()
                .fg(theme.fg)
                .bg(theme.bg_raised)
                .add_modifier(Modifier::BOLD),
        ),
        section,
    );

    match notes {
        UpdateNotesState::Loading => frame.render_widget(
            Paragraph::new("◌ Loading release notes…")
                .alignment(Alignment::Center)
                .style(theme.dim().bg(theme.bg_raised)),
            notes_text_area,
        ),
        UpdateNotesState::Failed => frame.render_widget(
            Paragraph::new("Release notes could not be loaded. You can still update Aven.")
                .alignment(Alignment::Center)
                .style(Style::new().fg(theme.warning).bg(theme.bg_raised)),
            notes_text_area,
        ),
        UpdateNotesState::Ready(markdown) => {
            let rendered = changelog_lines(markdown, content.width, theme);
            let start = clamp_scroll_start(scroll, rendered.len(), notes_area.height as usize);
            frame.render_widget(
                Paragraph::new(Text::from(
                    rendered
                        .iter()
                        .skip(start)
                        .take(notes_area.height as usize)
                        .cloned()
                        .collect::<Vec<_>>(),
                ))
                .style(Style::new().fg(theme.fg).bg(theme.bg_raised)),
                notes_text_area,
            );
            render_vertical_scrollbar(frame, theme, notes_area, rendered.len(), scroll);
        }
    }

    let primary_label = if guidance_has_command(plan) {
        " Copy command "
    } else if plan.guidance().is_some() {
        " Copy instructions "
    } else {
        " Update "
    };
    let later = action_span(theme, " Later ", focus == UpdateActionFocus::Later, false);
    let primary = action_span(
        theme,
        primary_label,
        focus == UpdateActionFocus::Primary,
        true,
    );
    let actions_width = (" Later ".len() + 1 + primary_label.len()) as u16;
    let hints_area = Rect {
        width: footer.width.saturating_sub(actions_width),
        ..footer
    };
    let actions_area = Rect {
        x: footer
            .x
            .saturating_add(footer.width.saturating_sub(actions_width)),
        width: actions_width,
        ..footer
    };
    frame.render_widget(
        Paragraph::new(dialog_hint_line(
            theme,
            &[("j/k", "scroll"), ("Tab", "action")],
        ))
        .style(theme.muted().bg(theme.bg_raised)),
        hints_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![later, Span::raw(" "), primary]))
            .alignment(Alignment::Right)
            .style(Style::new().bg(theme.bg_raised)),
        actions_area,
    );
}

fn guidance_has_command(plan: &crate::update::InstallPlan) -> bool {
    plan.guidance().is_some_and(|lines| {
        lines
            .iter()
            .any(|line| line.strip_prefix("Run: ").is_some())
    })
}

fn install_label(plan: &crate::update::InstallPlan) -> &'static str {
    match &plan.method {
        crate::update::InstallMethod::Direct { .. } => "Direct installation",
        crate::update::InstallMethod::Homebrew => "Managed by Homebrew",
        crate::update::InstallMethod::Cargo => "Managed by Cargo",
        crate::update::InstallMethod::Nix => "Managed by Nix",
        crate::update::InstallMethod::Development => "Development build",
        crate::update::InstallMethod::Unsupported { .. } => "Manual update required",
        crate::update::InstallMethod::Unwritable { .. } => "Install location is read-only",
    }
}

fn action_span(theme: &Theme, label: &'static str, focused: bool, primary: bool) -> Span<'static> {
    let fill = if focused {
        theme.accent
    } else {
        theme.bg_panel
    };
    let foreground = if focused {
        theme.inverse_fg
    } else if primary {
        theme.accent
    } else {
        theme.fg_muted
    };
    let mut style = if focused {
        theme.badge(fill)
    } else if primary {
        Style::new().fg(foreground).bg(fill)
    } else {
        theme.muted().bg(fill)
    };
    if focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    Span::styled(label, style)
}

fn update_title(state: &UpdateOverlayState) -> &'static str {
    match state {
        UpdateOverlayState::Available { .. } => "Software Update",
        UpdateOverlayState::Success { .. } => "Update installed",
        UpdateOverlayState::Failed { .. } => "Update failed",
        UpdateOverlayState::Cancelled => "Update cancelled",
        _ => "Update aven",
    }
}

fn update_lines(theme: &Theme, state: &UpdateOverlayState) -> Vec<Line<'static>> {
    match state {
        UpdateOverlayState::Available { .. } => Vec::new(),
        UpdateOverlayState::Checking => vec![
            status_line(
                theme,
                "●",
                "Checking GitHub for the latest stable release",
                theme.accent,
            ),
            Line::from(""),
            dialog_hint_line(theme, &[("Esc", "cancel")]),
        ],
        UpdateOverlayState::Progress {
            version,
            phase,
            cancelling,
            blockers,
        } => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled("target  ", theme.dim()),
                    Span::styled(format!("v{version}"), Style::new().fg(theme.accent)),
                ]),
                Line::from(""),
                status_line(
                    theme,
                    if *cancelling { "◌" } else { "●" },
                    if *cancelling {
                        "Cancelling update"
                    } else {
                        phase.label()
                    },
                    if *cancelling {
                        theme.warning
                    } else {
                        theme.accent
                    },
                ),
                Line::from(""),
            ];
            if !blockers.is_empty() {
                for blocker in blockers {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {:>7}  ", blocker.pid), theme.dim()),
                        Span::raw(blocker.command.clone()),
                    ]));
                }
                lines.push(Line::from(Span::styled(
                    "Quit these to continue. Aven does not stop them.",
                    theme.dim(),
                )));
                lines.push(Line::from(""));
            }
            lines.push(if phase.cancellable() {
                dialog_hint_line(theme, &[("Esc", "cancel")])
            } else {
                Line::from(Span::styled(
                    "Finishing the installation. Keep aven open.",
                    theme.dim(),
                ))
            });
            lines
        }
        UpdateOverlayState::Current { version, cached } => {
            let mut lines = vec![status_line(
                theme,
                "✓",
                &format!("Aven v{version} is up to date"),
                theme.success,
            )];
            if *cached {
                lines.push(cached_line(theme, true));
            }
            lines.push(Line::from(""));
            lines.push(dialog_hint_line(theme, &[("Esc", "close")]));
            lines
        }
        UpdateOverlayState::Success { version } => vec![
            status_line(
                theme,
                "✓",
                &format!("Installed aven v{version}"),
                theme.success,
            ),
            Line::from(""),
            Line::from("Restart aven to use the installed version."),
            Line::from("Your tasks and current database are unchanged."),
            Line::from(""),
            dialog_hint_line(theme, &[("q", "quit"), ("Esc", "continue")]),
        ],
        UpdateOverlayState::Failed { message } => vec![
            status_line(theme, "×", message, theme.danger),
            Line::from(""),
            Line::from(Span::styled(
                "The existing aven executable is unchanged.",
                theme.dim(),
            )),
            Line::from(""),
            dialog_hint_line(theme, &[("Enter", "retry"), ("Esc", "close")]),
        ],
        UpdateOverlayState::Cancelled => vec![
            status_line(theme, "○", "The update was cancelled", theme.warning),
            Line::from(""),
            Line::from("The existing aven executable is unchanged."),
            Line::from(""),
            dialog_hint_line(theme, &[("Enter", "try again"), ("Esc", "close")]),
        ],
    }
}

fn status_line(
    theme: &Theme,
    icon: &str,
    message: &str,
    color: ratatui::style::Color,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{icon} "),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(message.to_string(), Style::new().fg(theme.fg)),
    ])
}

fn cached_line(theme: &Theme, cached: bool) -> Line<'static> {
    if cached {
        Line::from(Span::styled(
            "Showing cached release information because a live check was unavailable.",
            theme.dim(),
        ))
    } else {
        Line::from("")
    }
}

#[cfg(test)]
pub(in crate::tui::ui) fn update_lines_for_test(state: &UpdateOverlayState) -> Vec<Line<'static>> {
    update_lines(&Theme::dark(), state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_action_uses_a_filled_accent_without_text_decoration() {
        let theme = Theme::dark();
        let action = action_span(&theme, " Update ", true, true);

        assert_eq!(action.style.bg, Some(theme.accent));
        assert_eq!(action.style.fg, Some(theme.inverse_fg));
        assert_eq!(action.style.add_modifier, Modifier::BOLD);
    }
}
