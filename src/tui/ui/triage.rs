use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use super::footer::{FooterMode, footer_bar, key_hint_segments, key_hint_spans};
use super::timestamps::local_timestamp_display;
use super::truncate::truncate_line_width;
use crate::choices::TaskStatus;
use crate::tui::event::{Action, CommandContext, preferred_shortcut_label};
use crate::tui::markdown::render_markdown;
use crate::tui::store::TuiStore;
use crate::tui::theme::Theme;
use crate::tui::triage::{TriageEntry, TriageOutcome, TriageSession};
use crate::tui::widgets::{priority_short, status_span};
use aven_core::queue::{TriageReason, untouched_since_creation};

const CARD_MAX_WIDTH: u16 = 88;
const RELATED_LIMIT: usize = 3;

/// Renders the triage screen. An open status or priority choice takes the
/// footer, as it does under the task list.
pub(super) fn render_triage(
    frame: &mut Frame,
    theme: &Theme,
    store: &TuiStore,
    session: &TriageSession,
    choice: Option<FooterMode>,
    custom_commands: bool,
) {
    let area = frame.area();
    frame.render_widget(Clear, area);
    frame.render_widget(Block::new().style(Style::new().bg(theme.bg)), area);
    let [header, _, body, _, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(frame, theme, header, store, session);

    let card_width = body.width.saturating_sub(4).min(CARD_MAX_WIDTH);
    let text_width = card_width.saturating_sub(6).max(1) as usize;
    let (header_lines, lines, actions) = match session.current() {
        Some(entry) => (
            card_header_lines(theme, entry, text_width),
            card_body_lines(theme, session, entry, text_width),
            wrap_segments(decision_segments(theme, entry, custom_commands), text_width),
        ),
        None => (
            Vec::new(),
            end_lines(theme, session),
            wrap_segments(end_segments(theme, session), text_width),
        ),
    };
    let wrapped_height = |lines: &[Line]| {
        lines
            .iter()
            .map(|line| line.width().div_ceil(text_width).max(1))
            .sum::<usize>() as u16
    };
    let header_height = wrapped_height(&header_lines);
    let text_height = wrapped_height(&lines);
    // The actions dock inside the card below a blank line and a rule.
    let dock_height = actions.len() as u16 + 1;
    let card_height =
        (header_height + text_height + dock_height + 3).min(body.height.saturating_sub(2));
    let card = Rect {
        x: body.x + (body.width - card_width) / 2,
        y: body.y + body.height.saturating_sub(card_height + 2) / 3,
        width: card_width,
        height: card_height,
    };
    let trail = Rect {
        y: card.bottom() + 1,
        height: 1,
        ..body
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.accent))
        .style(Style::new().bg(theme.bg_raised))
        .padding(Padding::horizontal(2));
    let inner = block.inner(card);
    frame.render_widget(block, card);
    let [sticky, content, _, rule, dock] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(dock_height.saturating_sub(1)),
    ])
    .areas(inner);
    session
        .scroll_max
        .set(text_height.saturating_sub(content.height));
    session.scroll_viewport.set(content.height);
    let scroll = session.card_scroll();
    frame.render_widget(
        Paragraph::new(header_lines).wrap(Wrap { trim: false }),
        sticky,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        content,
    );
    frame.render_widget(
        Paragraph::new(scroll_rule(
            theme,
            rule.width as usize,
            scroll,
            session.scroll_max.get(),
        )),
        rule,
    );
    frame.render_widget(Paragraph::new(actions), dock);

    if trail.bottom() <= body.bottom() {
        frame.render_widget(
            Paragraph::new(trail_line(theme, session, trail.width as usize))
                .alignment(Alignment::Center),
            trail,
        );
    }
    if let Some(mode) = choice {
        frame.render_widget(footer_bar(theme, mode, footer.width, 0), footer);
        return;
    }
    let hints: &[(&str, &str)] = if session.current().is_some() {
        &[
            ("j/k/←/→", "next/prev"),
            ("Enter", "detail"),
            ("u", "undo"),
            ("?", "more"),
            ("Esc", "leave"),
        ]
    } else {
        &[("k", "back"), ("u", "undo"), ("Esc", "leave")]
    };
    frame.render_widget(
        Paragraph::new(Line::from(key_hint_spans(theme, hints))).block(
            Block::new()
                .borders(Borders::TOP)
                .border_style(Style::new().fg(theme.border)),
        ),
        footer,
    );
}

/// The divider above the actions; when the body overflows it shows which way
/// there is more to read and the keys that scroll.
fn scroll_rule(theme: &Theme, width: usize, scroll: u16, max: u16) -> Line<'static> {
    let rule = Style::new().fg(theme.lane_divider);
    if max == 0 {
        return Line::from(Span::styled("─".repeat(width), rule));
    }
    let arrows = match (scroll > 0, scroll < max) {
        (true, true) => "↑↓",
        (true, false) => "↑",
        _ => "↓",
    };
    let label = vec![
        Span::styled(format!(" {arrows} "), Style::new().fg(theme.accent)),
        Span::styled("ctrl-d/u scroll ", Style::new().fg(theme.fg_dim)),
    ];
    let label_width = Line::from(label.clone()).width();
    let mut spans = vec![Span::styled(
        "─".repeat(width.saturating_sub(label_width + 2)),
        rule,
    )];
    spans.extend(label);
    spans.push(Span::styled("──", rule));
    Line::from(spans)
}

fn render_header(
    frame: &mut Frame,
    theme: &Theme,
    area: Rect,
    store: &TuiStore,
    session: &TriageSession,
) {
    let block = Block::new()
        .borders(Borders::BOTTOM)
        .border_style(Style::new().fg(theme.border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let scope = Line::from(vec![
        Span::styled(
            session.eligible_now().to_string(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" eligible", Style::new().fg(theme.fg_dim)),
        Span::styled(" │ ", Style::new().fg(theme.border)),
    ]);
    let mut scope = scope;
    match &session.project {
        Some(project) => scope.spans.extend([
            Span::styled("project ", Style::new().fg(theme.fg_dim)),
            Span::styled(project.clone(), Style::new().fg(theme.project(project))),
        ]),
        None => scope.spans.extend([
            Span::styled("ws ", Style::new().fg(theme.fg_dim)),
            Span::styled(
                store.active_workspace.key.clone(),
                Style::new().fg(theme.fg_muted),
            ),
        ]),
    }
    scope.spans.push(Span::raw(" "));
    let [left, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(scope.width() as u16),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(truncate_line_width(
            progress_line(theme, session),
            left.width as usize,
            Style::new().fg(theme.fg_dim),
        )),
        left,
    );
    frame.render_widget(Paragraph::new(scope), right);
}

/// One slot per card in the page: outcome-colored when decided, hollow when
/// passed over, a diamond for the current card, and a dot when not yet seen.
fn progress_line(theme: &Theme, session: &TriageSession) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            " TRIAGE ",
            Style::new()
                .fg(theme.inverse_fg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    for (index, entry) in session.entries.iter().enumerate() {
        let current = index == session.cursor;
        let (glyph, style) = match entry.outcome {
            Some(outcome) => (
                if current { "◆" } else { "●" },
                outcome_style(theme, outcome),
            ),
            None if current => ("◆", Style::new().fg(theme.accent)),
            None if entry.passed => ("○", Style::new().fg(theme.warning)),
            None => ("·", Style::new().fg(theme.border)),
        };
        spans.push(Span::styled(glyph, style));
    }
    let total = session.entries.len();
    let position = if session.at_end() {
        Span::styled("  page done", Style::new().fg(theme.success))
    } else {
        Span::styled(
            format!("  {}", session.cursor + 1),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        )
    };
    spans.push(position);
    if !session.at_end() {
        spans.push(Span::styled(
            format!("/{total}"),
            Style::new().fg(theme.fg_dim),
        ));
    }
    Line::from(spans)
}

fn outcome_style(theme: &Theme, outcome: TriageOutcome) -> Style {
    match outcome {
        TriageOutcome::Status(status) | TriageOutcome::Kept(status) => {
            theme.status(status.as_str())
        }
        TriageOutcome::Deferred => Style::new().fg(theme.attention),
        TriageOutcome::Deleted => Style::new().fg(theme.danger),
    }
}

fn reason_line(theme: &Theme, entry: &TriageEntry) -> Line<'static> {
    let task = &entry.item.task;
    let (label, color) = match entry.reason {
        TriageReason::Inbox => ("INBOX".to_string(), theme.info),
        TriageReason::StaleActive | TriageReason::StaleTodo => (
            format!("STALE {}", task.status.as_str().to_uppercase()),
            theme.warning,
        ),
    };
    let age = if untouched_since_creation(task) {
        format!("no activity since creation · {}d", entry.idle_days)
    } else {
        format!("last activity {}d", entry.idle_days)
    };
    let mut spans = vec![
        Span::styled(
            format!(" {label} "),
            Style::new()
                .fg(theme.inverse_fg)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {age}"), Style::new().fg(theme.fg_muted)),
    ];
    if let Some(outcome) = entry.outcome {
        spans.push(Span::styled(
            format!("   {}", outcome.label()),
            Style::new().fg(theme.success).add_modifier(Modifier::BOLD),
        ));
    }
    Line::from(spans)
}

fn meta_line(theme: &Theme, entry: &TriageEntry) -> Line<'static> {
    let task = &entry.item.task;
    let separator = || Span::raw("  ");
    let mut spans = vec![
        Span::styled(
            entry.item.display_ref.clone(),
            Style::new().fg(theme.fg_dim),
        ),
        separator(),
        Span::styled(
            task.project_key.clone(),
            Style::new().fg(theme.project(&task.project_key)),
        ),
        separator(),
        status_span(theme, task.status.as_str()),
        separator(),
        Span::styled(
            priority_short(task.priority.as_str()),
            theme.priority(task.priority.as_str()),
        ),
    ];
    if let Some(due) = &task.due_on {
        spans.push(separator());
        spans.push(Span::styled(
            format!("due {due}"),
            Style::new().fg(theme.warning),
        ));
    }
    if !entry.item.labels.is_empty() {
        spans.push(separator());
        spans.push(Span::styled(
            entry
                .item
                .labels
                .iter()
                .map(|label| format!("#{label}"))
                .collect::<Vec<_>>()
                .join(" "),
            Style::new().fg(theme.fg_dim),
        ));
    }
    Line::from(spans)
}

/// Evidence for the decision that the card itself does not show: what blocks
/// it and how loaded its project already is.
fn context_lines(
    theme: &Theme,
    session: &TriageSession,
    entry: &TriageEntry,
    width: usize,
) -> Vec<Line<'static>> {
    let label = |text: &str| Span::styled(format!("{text:<9}"), Style::new().fg(theme.fg_dim));
    let value = |text: String| Span::styled(text, Style::new().fg(theme.fg_muted));
    let mut lines = Vec::new();
    let blockers = entry
        .item
        .depends_on
        .iter()
        .filter(|link| !matches!(link.status.as_str(), "done" | "canceled"))
        .map(|link| format!("{} {}", link.display_ref, link.title))
        .collect::<Vec<_>>();
    if !blockers.is_empty() {
        lines.push(Line::from(vec![
            label("blocked"),
            Span::styled(blockers.join(" · "), Style::new().fg(theme.warning)),
        ]));
    }
    if let Some(counts) = session.project_counts.get(&entry.item.task.project_key) {
        lines.push(Line::from(vec![
            label("project"),
            value(format!(
                "{} · {} todo · {} active",
                entry.item.task.project_key, counts.todo, counts.active
            )),
        ]));
    }
    for (index, link) in entry.item.related.iter().take(RELATED_LIMIT).enumerate() {
        lines.push(Line::from(vec![
            label(if index == 0 { "related" } else { "" }),
            value(format!(
                "{} {} ({})",
                link.display_ref, link.title, link.status
            )),
        ]));
    }
    lines
        .into_iter()
        .map(|line| truncate_line_width(line, width, Style::new().fg(theme.fg_dim)))
        .collect()
}

/// The newest note, quoted like in the detail view.
fn latest_note_lines(theme: &Theme, entry: &TriageEntry, width: usize) -> Vec<Line<'static>> {
    let notes = &entry.item.notes;
    // Notes load newest first.
    let Some(note) = notes.first() else {
        return Vec::new();
    };
    let mut header = vec![
        Span::styled(
            "LATEST NOTE",
            Style::new().fg(theme.fg_dim).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {}", local_timestamp_display(&note.created_at)),
            Style::new().fg(theme.fg_dim),
        ),
    ];
    if notes.len() > 1 {
        header.push(Span::styled(
            format!(" · {} notes", notes.len()),
            Style::new().fg(theme.fg_dim),
        ));
    }
    let mut lines = vec![Line::raw(""), Line::from(header)];
    let body = render_markdown(theme, note.body.trim(), width.saturating_sub(2).max(1));
    for line in body {
        let mut spans = vec![Span::styled("│ ", Style::new().fg(theme.border))];
        spans.extend(line.spans);
        lines.push(Line::from(spans));
    }
    lines
}

/// The part of the card that stays put while the body scrolls.
fn card_header_lines(theme: &Theme, entry: &TriageEntry, width: usize) -> Vec<Line<'static>> {
    vec![
        reason_line(theme, entry),
        Line::raw(""),
        Line::from(Span::styled(
            entry.item.task.title.clone(),
            Style::new().fg(theme.fg).add_modifier(Modifier::BOLD),
        )),
        truncate_line_width(
            meta_line(theme, entry),
            width,
            Style::new().fg(theme.fg_dim),
        ),
        Line::raw(""),
    ]
}

fn card_body_lines(
    theme: &Theme,
    session: &TriageSession,
    entry: &TriageEntry,
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let description = entry.item.task.description.trim();
    if description.is_empty() {
        lines.push(Line::from(Span::styled(
            "No description.",
            Style::new().fg(theme.fg_dim).add_modifier(Modifier::ITALIC),
        )));
    } else {
        lines.extend(render_markdown(theme, description, width));
    }
    lines.extend(latest_note_lines(theme, entry, width));
    let context = context_lines(theme, session, entry, width);
    if !context.is_empty() {
        lines.push(Line::raw(""));
        lines.extend(context);
    }
    lines
}

fn end_lines(theme: &Theme, session: &TriageSession) -> Vec<Line<'static>> {
    let decided = session.decided_count();
    let undecided = session.entries.len() - decided;
    let mut lines = vec![
        Line::from(Span::styled(
            "Page complete",
            Style::new().fg(theme.success).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(
            format!("{decided} decided · {undecided} undecided"),
            Style::new().fg(theme.fg),
        )),
    ];
    if undecided > 0 {
        lines.push(Line::from(Span::styled(
            "Undecided tasks come back next session.",
            Style::new().fg(theme.fg_muted),
        )));
    }
    lines
}

fn end_segments(theme: &Theme, session: &TriageSession) -> Vec<Vec<Span<'static>>> {
    let next = format!("next {}", crate::tui::triage::PAGE_SIZE);
    if session.more_available {
        card_hints(theme, &[("Enter", &next), ("Esc", "leave")])
    } else {
        card_hints(theme, &[("Esc", "leave")])
    }
}

fn card_hints(theme: &Theme, hints: &[(&str, &str)]) -> Vec<Vec<Span<'static>>> {
    key_hint_segments(theme, hints, theme.bg_raised, theme.lane_divider)
}

/// Lays hint segments out left to right, starting a new line instead of
/// splitting a segment.
fn wrap_segments(segments: Vec<Vec<Span<'static>>>, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut used = 0;
    for segment in segments {
        let segment_width = segment.iter().map(Span::width).sum::<usize>();
        if !current.is_empty() && used + segment_width > width {
            lines.push(Line::from(std::mem::take(&mut current)));
            used = 0;
        }
        used += segment_width;
        current.extend(segment);
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

/// Decisions labeled with the task list's own shortcuts, so the keys learned
/// here are the keys used everywhere else.
fn decision_segments(
    theme: &Theme,
    entry: &TriageEntry,
    custom_commands: bool,
) -> Vec<Vec<Span<'static>>> {
    let current = entry.item.task.status;
    let status_label = |status: TaskStatus| {
        if status != current || current == TaskStatus::Inbox {
            status.to_string()
        } else if status == TaskStatus::Active {
            "still active".to_string()
        } else {
            format!("keep {status}")
        }
    };
    let mut decisions = vec![
        (
            Action::SetStatus(TaskStatus::Todo),
            status_label(TaskStatus::Todo),
        ),
        (
            Action::SetStatus(TaskStatus::Backlog),
            status_label(TaskStatus::Backlog),
        ),
    ];
    if current == TaskStatus::Active {
        decisions.push((
            Action::SetStatus(TaskStatus::Active),
            status_label(TaskStatus::Active),
        ));
    }
    decisions.extend([
        (Action::SetStatus(TaskStatus::Done), "done".to_string()),
        (
            Action::SetStatus(TaskStatus::Canceled),
            "cancel".to_string(),
        ),
        (Action::BeginEditAvailability, "available…".to_string()),
        (Action::BeginEditPriority, "priority…".to_string()),
    ]);
    let mut hints = decisions
        .iter()
        .filter_map(|(action, label)| {
            preferred_shortcut_label(*action, CommandContext::Normal)
                .map(|key| (key, label.as_str()))
        })
        .collect::<Vec<_>>();
    if custom_commands {
        hints.push(("z", "commands…"));
    }
    card_hints(theme, &hints)
}

fn trail_line(theme: &Theme, session: &TriageSession, width: usize) -> Line<'static> {
    let mut spans = Vec::new();
    for entry in session.recent_decisions().take(3) {
        let Some(outcome) = entry.outcome else {
            continue;
        };
        let style = if spans.is_empty() {
            Style::new().fg(theme.fg_muted)
        } else {
            spans.push(Span::raw("   "));
            Style::new().fg(theme.fg_dim)
        };
        spans.push(Span::styled(
            format!("{} {}", entry.item.display_ref, outcome.label()),
            style,
        ));
    }
    if spans.is_empty() {
        return Line::raw("");
    }
    spans.insert(0, Span::styled("last  ", Style::new().fg(theme.fg_dim)));
    truncate_line_width(Line::from(spans), width, Style::new().fg(theme.fg_dim))
}
