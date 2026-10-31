//! The minimal window that proves capture and measurement end to end.
use std::path::Path;

use anyhow::Result;
use aven_core::api::{QueueTaskSummary, Store};
use aven_core::ids::WorkspaceId;
use aven_core::queue::QueueBand;
use chrono::{DateTime, Local, NaiveDate};
use gpui::{
    Context as GpuiContext, Div, IntoElement, ParentElement, Render, Rgba, SharedString, Styled,
    Window, div, px, rgb,
};

use crate::measure::measured;

const NAV_WIDTH: f32 = 228.;
const DETAIL_WIDTH: f32 = 380.;
/// The prototype's resize handles overlap each neighbouring panel by this much.
const SPLIT_OVERLAP: f32 = 1.;
/// At or below this window width, the prototype starts with no detail panel.
const DETAIL_MIN_WINDOW_WIDTH: f32 = 1180.;
/// Groups in display order, with the prototype's names.
const BANDS: [(QueueBand, &str); 8] = [
    (QueueBand::NeedsAction, "Needs action"),
    (QueueBand::Available, "Available"),
    (QueueBand::Focus, "Focus"),
    (QueueBand::Soon, "Medium priority"),
    (QueueBand::Triage, "Triage"),
    (QueueBand::Blocked, "Blocked"),
    (QueueBand::Later, "Later"),
    (QueueBand::Epics, "Epics"),
];

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    chrome: Rgba,
    page: Rgba,
    ink: Rgba,
    body: Rgba,
    muted: Rgba,
    faint: Rgba,
    line: Rgba,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            chrome: rgb(0x100f0e),
            page: rgb(0x181716),
            ink: rgb(0xeeece6),
            body: rgb(0xc3c0b8),
            muted: rgb(0xaca69e),
            faint: rgb(0x918b83),
            line: rgb(0x2c2a27),
        }
    }

    pub fn light() -> Self {
        Self {
            chrome: rgb(0xefece6),
            page: rgb(0xfdfcfa),
            ink: rgb(0x1a1816),
            body: rgb(0x3f3b37),
            muted: rgb(0x68625c),
            faint: rgb(0x756e67),
            line: rgb(0xe7e2d9),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub reference: SharedString,
    pub title: SharedString,
    pub due: SharedString,
}

#[derive(Debug, Clone)]
pub struct Group {
    pub name: &'static str,
    pub rows: Vec<Row>,
}

/// Reads a workspace's queue, by default the database's selected one. `now`
/// formats dates, so scene runs pass their anchor.
pub async fn load_groups(
    path: &Path,
    workspace: Option<&WorkspaceId>,
    now: DateTime<Local>,
) -> Result<Vec<Group>> {
    let store = Store::open(path).await.map_err(anyhow::Error::msg)?;
    let selected = match workspace {
        Some(workspace) => workspace.clone(),
        None => {
            let state = store.queue_state().await.map_err(anyhow::Error::msg)?;
            state.selected_workspace.id
        }
    };
    let report = store
        .queue_report(&selected)
        .await
        .map_err(anyhow::Error::msg)?;
    store.close().await;
    Ok(group(&report.tasks, now.date_naive()))
}

fn group(tasks: &[QueueTaskSummary], today: NaiveDate) -> Vec<Group> {
    BANDS
        .iter()
        .filter_map(|(band, name)| {
            let rows: Vec<_> = tasks
                .iter()
                .filter(|task| task.band == *band)
                .map(|task| Row {
                    reference: task.display_ref.clone().into(),
                    title: task.title.clone().into(),
                    due: task
                        .due_on
                        .as_deref()
                        .map(|due| due_text(due, today))
                        .unwrap_or_default()
                        .into(),
                })
                .collect();
            (!rows.is_empty()).then_some(Group { name, rows })
        })
        .collect()
}

/// Matches the prototype's `dueText`.
pub fn due_text(due: &str, today: NaiveDate) -> String {
    let Ok(date) = NaiveDate::parse_from_str(due, "%Y-%m-%d") else {
        return due.to_string();
    };
    match (date - today).num_days() {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => date.format("%b %-d").to_string(),
    }
}

pub struct Shell {
    pub palette: Palette,
    pub groups: Vec<Group>,
}

/// A section heading, whose 16.8px line GPUI cannot place exactly.
const SECTION_HEIGHT: f32 = 32.8;

/// GPUI snaps lengths to device pixels. Headings carry the remainder from one
/// to the next, so the rows below them do not drift down the list.
fn snapped_height(height: f32) -> f32 {
    (height * 2.).round() / 2.
}

/// A border drawn as an element. A real border insets the measurement probe,
/// so measured elements draw theirs this way.
fn hairline(palette: Palette) -> Div {
    div().absolute().bg(palette.line)
}

impl Shell {
    fn list(&self) -> Div {
        let palette = self.palette;
        let mut list = measured(div(), "list")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .pt(px(4.))
            .px(px(8.))
            .pb(px(40.));
        let mut row_index = 0;
        let mut carried = 0.;
        for (group_index, group) in self.groups.iter().enumerate() {
            let height = snapped_height(SECTION_HEIGHT + carried);
            carried += SECTION_HEIGHT - height;
            list = list.child(
                measured(div(), format!("section-{group_index}"))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .h(px(height))
                    .pt(px(12.))
                    .pb(px(4.))
                    .px(px(8.))
                    .text_size(px(12.))
                    .line_height(px(16.8))
                    .text_color(palette.muted)
                    .child(group.name)
                    .child(
                        div()
                            .text_color(palette.faint)
                            .child(group.rows.len().to_string()),
                    ),
            );
            for row in &group.rows {
                list = list.child(
                    measured(div(), format!("row-{row_index}"))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(9.))
                        .h(px(31.))
                        .px(px(8.))
                        .text_color(palette.body)
                        .child(div().text_color(palette.faint).child(row.reference.clone()))
                        .child(div().flex_1().min_w_0().truncate().child(row.title.clone()))
                        .child(div().text_color(palette.faint).child(row.due.clone())),
                );
                row_index += 1;
            }
        }
        list
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, _: &mut GpuiContext<Self>) -> impl IntoElement {
        let palette = self.palette;
        let detail = (window.viewport_size().width > px(DETAIL_MIN_WINDOW_WIDTH)).then(|| {
            measured(div(), "detail")
                .flex_none()
                .w(px(DETAIL_WIDTH))
                .h_full()
                .ml(px(-SPLIT_OVERLAP))
                .child(hairline(palette).top_0().left_0().h_full().w(px(1.)))
        });
        measured(div(), "app")
            .size_full()
            .flex()
            .bg(palette.chrome)
            .text_size(px(13.))
            .line_height(px(18.2))
            .text_color(palette.ink)
            .child(measured(div(), "nav").flex_none().w(px(NAV_WIDTH)).h_full())
            .child(
                measured(div(), "main")
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .ml(px(-SPLIT_OVERLAP))
                    .mt(px(6.))
                    .mr(px(6.))
                    .mb(px(6.))
                    .rounded(px(10.))
                    .bg(palette.page)
                    .overflow_hidden()
                    .child(
                        measured(div(), "main-header")
                            .flex_none()
                            .flex()
                            .items_center()
                            .h(px(44.))
                            .pl(px(16.))
                            .pr(px(12.))
                            .child("Queue")
                            .child(hairline(palette).bottom_0().left_0().w_full().h(px(1.))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(self.list()),
                            )
                            .children(detail),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_text_matches_the_prototype_around_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        for (due, expected) in [
            ("2026-10-14", "Today"),
            ("2026-10-15", "Tomorrow"),
            ("2026-10-13", "Yesterday"),
            ("2026-10-02", "Oct 2"),
            ("2026-11-20", "Nov 20"),
        ] {
            assert_eq!(due_text(due, today), expected);
        }
    }
}
