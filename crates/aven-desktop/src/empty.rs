//! What an empty list says. Emptiness reads by what it means: a view cleared
//! of work earns the logo, a search or filter miss sits as a notice where the
//! rows would be, and every other view says quietly what would fill it.
use std::time::Duration;

use aven_core::query::RecurrenceSeriesLifecycleFilter as Lifecycle;
use aven_host::store::TaskQuery;
use gpui::{Animation, AnimationExt, AnyElement, Context, FontWeight, div, img, prelude::*};

use crate::actions::*;
use crate::adapter::{Snapshot, SyncPhase};
use crate::commands::view_name;
use crate::kit::*;
use crate::shell::Root;
use crate::theme::{Theme, theme};

const ENTER_DURATION: Duration = Duration::from_millis(320);

/// What a button on an empty view does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    NewTask,
    NewEpic,
    NewRecurring,
    ShowClosed,
    Search,
    ClearFilters,
    SyncNow,
    OpenSync,
    View(TaskQuery),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Button {
    act: Act,
    label: String,
    keys: Option<&'static str>,
    /// The tasks waiting in the view the button opens.
    count: Option<i64>,
    fill: bool,
}

impl Button {
    fn fill(act: Act, label: &str, keys: Option<&'static str>) -> Self {
        let label = label.to_string();
        Self {
            act,
            label,
            keys,
            count: None,
            fill: true,
        }
    }

    fn line(act: Act, label: &str) -> Self {
        let label = label.to_string();
        Self {
            act,
            label,
            keys: None,
            count: None,
            fill: false,
        }
    }
}

/// A supporting line that may name a shortcut in the middle.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Line {
    text: String,
    keys: Option<(&'static str, &'static str)>,
}

impl Line {
    fn plain(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            keys: None,
        }
    }

    fn keys(text: &str, keys: &'static str, tail: &'static str) -> Self {
        Self {
            text: text.to_string(),
            keys: Some((keys, tail)),
        }
    }
}

/// A count beneath a cleared view that leads to where the work went.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stat {
    count: i64,
    label: &'static str,
    view: TaskQuery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Empty {
    /// A view whose emptiness is good news.
    Cleared {
        title: &'static str,
        line: Line,
        stats: Vec<Stat>,
        buttons: Vec<Button>,
    },
    /// A workspace or project that has never had a task.
    Blank { title: String, buttons: Vec<Button> },
    /// A search or filter that matched nothing.
    Miss {
        search: bool,
        title: String,
        line: String,
        buttons: Vec<Button>,
    },
    /// Any other view, which says what would fill it.
    Quiet {
        title: &'static str,
        line: Line,
        buttons: Vec<Button>,
    },
}

fn describe(snapshot: &Snapshot) -> Empty {
    let view = &snapshot.view;
    let count = |key: &str| {
        let counts = snapshot.views.iter();
        counts
            .clone()
            .find(|(view, _)| *view == key)
            .map_or(0, |(_, n)| *n)
    };
    let scope = view.project.as_ref().and_then(|key| {
        let projects = snapshot.projects.iter();
        projects
            .clone()
            .find(|project| project.key == *key)
            .map(|project| project.name.clone())
    });
    let within = scope
        .as_ref()
        .map_or(String::new(), |name| format!(" in {name}"));
    let go = |query: TaskQuery| {
        let n = count(count_key(query));
        Button {
            count: (n > 0).then_some(n),
            ..Button::line(Act::View(query), view_name(query))
        }
    };
    if view.query == TaskQuery::Search {
        let text = view.search.clone().unwrap_or_default();
        let place = scope.clone().unwrap_or("all projects".to_string());
        return Empty::Miss {
            search: true,
            title: format!("No results for “{text}”"),
            line: format!("Searched {place}. Try fewer or different words."),
            buttons: vec![
                Button::fill(Act::Search, "Refine search", Some("/")),
                Button::line(Act::View(TaskQuery::Queue), "Back to Queue"),
            ],
        };
    }
    if view.has_filters() {
        return Empty::Miss {
            search: false,
            title: "No tasks match these filters".to_string(),
            line: format!("Filters hide everything in {}.", view_name(view.query)),
            buttons: vec![Button::fill(
                Act::ClearFilters,
                "Clear filters",
                Some("f c"),
            )],
        };
    }
    let blank = count("queue") + count("upcoming") + count("done") == 0;
    let history = matches!(
        view.query,
        TaskQuery::Recurring | TaskQuery::RecentActions | TaskQuery::Conflicts
    );
    if blank && !history {
        let place = scope.clone().unwrap_or(snapshot.workspace.clone());
        let mut buttons = vec![Button::fill(Act::NewTask, "New task", Some("a"))];
        if scope.is_none() && snapshot.sync.phase == SyncPhase::Local {
            buttons.push(Button::line(Act::OpenSync, "Sync from another device"));
        }
        return Empty::Blank {
            title: format!("No tasks in {place} yet"),
            buttons,
        };
    }
    let cleared = |title, line: Line, stats: &[TaskQuery], buttons| {
        let stats = stats.iter().map(|query| Stat {
            count: count(count_key(*query)),
            label: stat_label(*query),
            view: *query,
        });
        Empty::Cleared {
            title,
            line,
            stats: stats.collect(),
            buttons,
        }
    };
    let quiet = |title, line, buttons| Empty::Quiet {
        title,
        line,
        buttons,
    };
    match view.query {
        TaskQuery::Queue => cleared(
            "All clear",
            Line::plain(format!("Nothing open is available{within} right now.")),
            &[TaskQuery::Done, TaskQuery::Upcoming],
            Vec::new(),
        ),
        TaskQuery::Inbox => cleared(
            "Inbox zero",
            Line::plain("Everything captured has been triaged."),
            &[TaskQuery::Done, TaskQuery::Backlog, TaskQuery::Todo],
            Vec::new(),
        ),
        TaskQuery::Overdue if view.due_today => cleared(
            "Nothing due",
            Line::plain(format!("Nothing open{within} is due today or overdue.")),
            &[TaskQuery::Done, TaskQuery::Upcoming],
            Vec::new(),
        ),
        TaskQuery::Overdue => cleared(
            "Nothing overdue",
            Line::plain("Deferred work with a passed deadline stays in Upcoming."),
            &[TaskQuery::Done, TaskQuery::Upcoming],
            Vec::new(),
        ),
        TaskQuery::Blocked => cleared(
            "Nothing is blocked",
            Line::plain(format!("Every open task{within} can start.")),
            &[TaskQuery::Done, TaskQuery::Ready],
            Vec::new(),
        ),
        TaskQuery::Conflicts => cleared(
            "Devices agree",
            Line::plain("Every field matches across your devices."),
            &[],
            vec![Button::line(Act::SyncNow, "Sync now")],
        ),
        TaskQuery::Ready => {
            let (blocked, deferred) = (count("blocked"), count("upcoming"));
            let line = if blocked + deferred > 0 {
                format!("{blocked} blocked and {deferred} deferred{within}.")
            } else {
                format!("Nothing is blocked or deferred{within} either.")
            };
            let go = vec![go(TaskQuery::Blocked), go(TaskQuery::Upcoming)];
            quiet("Nothing is ready", Line::plain(line), go)
        }
        TaskQuery::Upcoming => quiet(
            "Nothing deferred",
            Line::keys("Defer a task with", "e a", "to hide it until it’s needed."),
            vec![go(TaskQuery::Queue)],
        ),
        TaskQuery::Epics => {
            let mut buttons = vec![Button::fill(Act::NewEpic, "New epic", None)];
            if !view.closed {
                buttons.push(Button::line(Act::ShowClosed, "Show closed"));
            }
            let title = if view.closed {
                "No epics"
            } else {
                "No open epics"
            };
            let line = Line::plain("Epics group related tasks and track their progress.");
            quiet(title, line, buttons)
        }
        TaskQuery::Recurring => {
            let (title, line) = match view.lifecycle {
                Lifecycle::Active => (
                    "No active recurring tasks",
                    "Give a task a schedule, like every Monday, and it repeats.",
                ),
                Lifecycle::Paused => (
                    "Nothing paused",
                    "Paused series hide their current task until resumed.",
                ),
                Lifecycle::Stopped => {
                    ("Nothing stopped", "Stopped series keep their history here.")
                }
                Lifecycle::ActiveOrPaused | Lifecycle::All => (
                    "No recurring tasks",
                    "Give a task a schedule, like every Monday, and it repeats.",
                ),
            };
            let new = Button::fill(Act::NewRecurring, "New recurring task", None);
            quiet(title, Line::plain(line), vec![new])
        }
        TaskQuery::Done => quiet(
            "Nothing done yet",
            Line::plain(format!("Finished and canceled tasks{within} collect here.")),
            vec![go(TaskQuery::Queue)],
        ),
        TaskQuery::Open => quiet(
            "No open tasks",
            Line::plain(format!("Everything{within} is done, canceled or deferred.")),
            vec![go(TaskQuery::Upcoming), go(TaskQuery::Done)],
        ),
        TaskQuery::Backlog => quiet(
            "Backlog is empty",
            Line::keys("Park a task for later with", "t b", "."),
            vec![go(TaskQuery::Inbox), go(TaskQuery::Todo)],
        ),
        TaskQuery::Todo => quiet(
            "Nothing in Todo",
            Line::keys("Commit to a task with", "t t", "."),
            vec![go(TaskQuery::Backlog), go(TaskQuery::Ready)],
        ),
        TaskQuery::Active => quiet(
            "Nothing in progress",
            Line::keys("Start a task with", "t a", "."),
            vec![go(TaskQuery::Todo), go(TaskQuery::Ready)],
        ),
        TaskQuery::RecentActions => quiet(
            "No recent actions",
            Line::plain("Changes to tasks show up here, newest first."),
            Vec::new(),
        ),
        _ => quiet("Nothing here", Line::default(), Vec::new()),
    }
}

/// The key a view's count goes by in the snapshot. Unlike the sidebar's, it
/// covers every view the snapshot counts.
fn count_key(query: TaskQuery) -> &'static str {
    match query {
        TaskQuery::Queue => "queue",
        TaskQuery::Ready => "ready",
        TaskQuery::Inbox => "inbox",
        TaskQuery::Upcoming => "upcoming",
        TaskQuery::Overdue => "overdue",
        TaskQuery::Blocked => "blocked",
        TaskQuery::Epics => "epics",
        TaskQuery::Recurring => "recurring",
        TaskQuery::Conflicts => "conflicts",
        TaskQuery::Active => "active",
        TaskQuery::Done => "done",
        TaskQuery::Backlog => "backlog",
        TaskQuery::Todo => "todo",
        _ => "",
    }
}

fn stat_label(query: TaskQuery) -> &'static str {
    match query {
        TaskQuery::Done => "done",
        TaskQuery::Upcoming => "upcoming",
        TaskQuery::Backlog => "in backlog",
        TaskQuery::Todo => "to do",
        TaskQuery::Ready => "ready",
        _ => "",
    }
}

/// A shortcut set into running text.
fn keycap(keys: &'static str, t: &Theme) -> impl IntoElement + use<> {
    div()
        .flex_none()
        .px(px(4.))
        .rounded(px(4.))
        .bg(t.raised)
        .shadow(vec![t.inset_ring(t.line2)])
        .text(Text::Caption)
        .text_color(t.muted)
        .child(keys)
}

/// The line, with its shortcut as a key cap. Punctuation stays on the cap.
fn line_element(line: Line, t: &Theme) -> impl IntoElement + use<> {
    let row = div()
        .flex()
        .flex_wrap()
        .justify_center()
        .items_center()
        .gap(px(4.))
        .child(line.text);
    let Some((keys, tail)) = line.keys else {
        return row;
    };
    let cap = div().flex().items_center().child(keycap(keys, t));
    if tail.starts_with(char::is_alphanumeric) {
        row.child(cap).child(tail)
    } else {
        row.child(cap.child(tail))
    }
}

impl Root {
    pub(crate) fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        let snapshot = &self.session.snapshot;
        let state = describe(snapshot);
        let query = snapshot.view.query;
        let buttons = |buttons: Vec<Button>, cx: &mut Context<Self>| {
            let row = div().flex().flex_wrap().justify_center().gap(px(6.));
            buttons
                .into_iter()
                .enumerate()
                .fold(row, |row, (ix, button)| {
                    row.child(self.empty_button(ix, button, &t, cx))
                })
        };
        match state {
            Empty::Miss {
                search,
                title,
                line,
                buttons: actions,
            } => {
                let path = if search {
                    "icons/ic-search.svg"
                } else {
                    "icons/ic-settings.svg"
                };
                let notice = div()
                    .w_full()
                    .px(px(12.))
                    .py(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .rounded(px(8.))
                    .bg(t.warn.opacity(0.12))
                    .child(icon(path, t.warn))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.ink)
                                    .child(title),
                            )
                            .child(small(Text::Secondary).text_color(t.muted).child(line)),
                    )
                    .child(buttons(actions, cx));
                div()
                    .w_full()
                    .px(px(8.))
                    .pt(px(10.))
                    .child(notice)
                    .into_any_element()
            }
            Empty::Cleared {
                title,
                line,
                stats,
                buttons: actions,
            } => {
                let has_stats = !stats.is_empty();
                let stats = stats.into_iter().enumerate().fold(
                    div()
                        .mt(px(18.))
                        .pt(px(14.))
                        .flex()
                        .gap(px(18.))
                        .border_t_1()
                        .border_color(t.line),
                    |row, (ix, stat)| row.child(self.empty_stat(ix, stat, &t, cx)),
                );
                let body = self
                    .empty_mark(&t)
                    .child(
                        div()
                            .mt(px(14.))
                            .text(Text::Title)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(title),
                    )
                    .child(div().mt(px(5.)).child(line_element(line, &t)))
                    .when(!actions.is_empty(), |body| {
                        body.child(div().mt(px(16.)).child(buttons(actions, cx)))
                    })
                    .when(has_stats, |body| body.child(stats));
                self.empty_enter(query, body)
            }
            Empty::Blank {
                title,
                buttons: actions,
            } => {
                let body = self
                    .empty_mark(&t)
                    .child(
                        div()
                            .mt(px(14.))
                            .text(Text::Title)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.ink)
                            .child(title),
                    )
                    .child(
                        div()
                            .mt(px(5.))
                            .child("Every view fills in once you add a task."),
                    )
                    .child(div().mt(px(16.)).child(buttons(actions, cx)));
                self.empty_enter(query, body)
            }
            Empty::Quiet {
                title,
                line,
                buttons: actions,
            } => div()
                .w_full()
                .pt(px(56.))
                .flex()
                .justify_center()
                .child(
                    div()
                        .max_w(px(360.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .text_color(t.faint)
                        .child(
                            div()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.muted)
                                .child(title),
                        )
                        .when(!line.text.is_empty(), |empty| {
                            empty.child(div().mt(px(3.)).child(line_element(line, &t)))
                        })
                        .when(!actions.is_empty(), |empty| {
                            empty.child(div().mt(px(12.)).child(buttons(actions, cx)))
                        }),
                )
                .into_any_element(),
        }
    }

    /// The logo column a cleared or blank view stands on.
    fn empty_mark(&self, t: &Theme) -> gpui::Div {
        let logo = if t.dark {
            "logo-dark.svg"
        } else {
            "logo-light.svg"
        };
        div()
            .max_w(px(380.))
            .flex()
            .flex_col()
            .items_center()
            .text_color(t.muted)
            .child(img(logo).size(px(44.)))
    }

    /// Settles the column into place as the view opens.
    fn empty_enter(&self, query: TaskQuery, body: gpui::Div) -> AnyElement {
        let frame = div().w_full().pt(px(64.)).flex().justify_center();
        if self.session.motion.reduced() {
            return frame.child(body).into_any_element();
        }
        let id = VIEWS.iter().position(|view| view.0 == query).unwrap_or(0);
        frame
            .child(div().relative().child(body).with_animation(
                ("empty-enter", id),
                Animation::new(ENTER_DURATION),
                |body, delta| {
                    let shown = ease_out(delta);
                    body.top(px(6. * (1. - shown))).opacity(shown)
                },
            ))
            .into_any_element()
    }

    fn empty_stat(
        &self,
        ix: usize,
        stat: Stat,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = stat.view;
        let panel = t.panel;
        bare_btn(("empty-stat", ix), view_name(view), t)
            .h_auto()
            .py(px(4.))
            .px(px(8.))
            .flex_col()
            .gap(px(0.))
            .hover(move |style| style.bg(panel))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.show_view(&ShowView(view), window, cx);
            }))
            .child(
                div()
                    .text(Text::Title)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.ink)
                    .child(stat.count.to_string()),
            )
            .child(small(Text::Secondary).text_color(t.faint).child(stat.label))
    }

    fn empty_button(
        &self,
        ix: usize,
        button: Button,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let act = button.act;
        let base = if button.fill {
            fill_btn(("empty-act", ix), button.label, t)
        } else {
            line_btn(("empty-act", ix), button.label, t)
        };
        let keys = if button.fill {
            t.on_fill.opacity(0.7)
        } else {
            t.faint
        };
        base.when_some(button.count, |base, count| {
            base.child(div().text_color(t.faint).child(count.to_string()))
        })
        .when_some(button.keys, |base, text| base.child(kbd(text, keys)))
        .on_click(cx.listener(move |this, _, window, cx| match act {
            Act::NewTask => this.open_composer(window, cx),
            Act::NewEpic => this.new_epic(&NewEpic, window, cx),
            Act::NewRecurring => this.new_recurring(window, cx),
            Act::ShowClosed => this.toggle_closed(window, cx),
            Act::Search => this.open_search(&OpenSearch, window, cx),
            Act::ClearFilters => this.clear_filters(&ClearFilters, window, cx),
            Act::SyncNow => this.sync_now(&SyncNow, window, cx),
            Act::OpenSync => this.open_sync(&OpenSync, window, cx),
            Act::View(query) => this.show_view(&ShowView(query), window, cx),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::{NavEntry, ViewInfo};

    fn snapshot(query: TaskQuery, views: &[(&'static str, i64)]) -> Snapshot {
        Snapshot {
            workspace: "Personal".to_string(),
            views: views.to_vec(),
            view: ViewInfo {
                query,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_cleared_queue_counts_where_the_work_went() {
        let state = describe(&snapshot(
            TaskQuery::Queue,
            &[("upcoming", 4), ("done", 12)],
        ));
        let Empty::Cleared { title, stats, .. } = state else {
            panic!("expected a cleared queue, got {state:?}");
        };
        assert_eq!(title, "All clear");
        let counts: Vec<_> = stats.iter().map(|stat| (stat.label, stat.count)).collect();
        assert_eq!(counts, [("done", 12), ("upcoming", 4)]);
    }

    #[test]
    fn agreeing_devices_skip_the_counts() {
        let state = describe(&snapshot(
            TaskQuery::Conflicts,
            &[("queue", 2), ("done", 5)],
        ));
        let Empty::Cleared { stats, buttons, .. } = state else {
            panic!("expected cleared conflicts, got {state:?}");
        };
        assert!(stats.is_empty());
        assert_eq!(buttons[0].act, Act::SyncNow);
    }

    #[test]
    fn a_workspace_without_tasks_invites_the_first() {
        let state = describe(&snapshot(TaskQuery::Done, &[]));
        let Empty::Blank { title, buttons } = state else {
            panic!("expected a blank workspace, got {state:?}");
        };
        assert_eq!(title, "No tasks in Personal yet");
        assert_eq!(buttons[0].act, Act::NewTask);
        assert_eq!(buttons[1].act, Act::OpenSync);
    }

    #[test]
    fn a_blank_project_names_the_project_and_skips_sync() {
        let mut list = snapshot(TaskQuery::Queue, &[]);
        list.view.project = Some("aven".to_string());
        list.projects = vec![NavEntry {
            name: "Aven".to_string(),
            key: "aven".to_string(),
            prefix: "AVN".to_string(),
            count: 0,
        }];
        let Empty::Blank { title, buttons } = describe(&list) else {
            panic!("expected a blank project");
        };
        assert_eq!(title, "No tasks in Aven yet");
        assert_eq!(buttons.len(), 1);
    }

    #[test]
    fn history_views_keep_their_own_copy_when_blank() {
        let state = describe(&snapshot(TaskQuery::RecentActions, &[]));
        assert!(matches!(
            state,
            Empty::Quiet {
                title: "No recent actions",
                ..
            }
        ));
    }

    #[test]
    fn a_search_miss_names_the_query() {
        let mut list = snapshot(TaskQuery::Search, &[("done", 3)]);
        list.view.search = Some("invoice export".to_string());
        let Empty::Miss { search, title, .. } = describe(&list) else {
            panic!("expected a miss");
        };
        assert!(search);
        assert_eq!(title, "No results for “invoice export”");
    }

    #[test]
    fn filters_take_precedence_over_the_view() {
        let mut list = snapshot(TaskQuery::Queue, &[("done", 3)]);
        list.view.label = Some("billing".to_string());
        let state = describe(&list);
        assert!(matches!(state, Empty::Miss { search: false, .. }));
    }

    #[test]
    fn ready_says_what_holds_the_work_back() {
        let views = [("queue", 5), ("blocked", 2), ("upcoming", 3)];
        let Empty::Quiet { line, buttons, .. } = describe(&snapshot(TaskQuery::Ready, &views))
        else {
            panic!("expected a quiet view");
        };
        assert_eq!(line.text, "2 blocked and 3 deferred.");
        let links: Vec<_> = buttons
            .iter()
            .map(|button| (button.label.as_str(), button.count))
            .collect();
        assert_eq!(links, [("Blocked", Some(2)), ("Upcoming", Some(3))]);
    }
}
