//! The Recent actions ledger: one row per run of actions on a task, under day
//! and burst headings. A single action reads in full; a run reads as its last
//! few steps in order, oldest first.
use accesskit::Role;
use gpui::{AnyElement, Context, Div, FontWeight, Window, div, prelude::*};

use crate::adapter::{LedgerLine, LedgerRun, LedgerStep, StepMark};
use crate::kit::*;
use crate::shell::Root;
use crate::theme::{Theme, theme};

pub(crate) const LEDGER_DAY_HEIGHT: f32 = 38.;
pub(crate) const LEDGER_BURST_HEIGHT: f32 = 30.;
pub(crate) const LEDGER_ROW_HEIGHT: f32 = 31.;
const CLOCK_WIDTH: f32 = 38.;
/// A run shows at most this many steps; older ones collapse into a count.
const RUN_STEPS: usize = 4;

impl Root {
    pub(crate) fn render_ledger_line(&self, line: usize, cx: &mut Context<Self>) -> AnyElement {
        let t = theme(cx);
        match &self.session.snapshot.recent.lines[line] {
            LedgerLine::Day { label, count } => heading(LEDGER_DAY_HEIGHT, &t)
                .pt(px(14.))
                .text_color(t.ink)
                .font_weight(FontWeight::SEMIBOLD)
                .child(label.clone())
                .child(aside(plural(*count, "action"), &t))
                .into_any_element(),
            LedgerLine::Burst { label, range } => heading(LEDGER_BURST_HEIGHT, &t)
                .pt(px(10.))
                .text_color(t.muted)
                .font_weight(FontWeight::MEDIUM)
                .child(label.clone())
                .child(aside(range.clone(), &t))
                .into_any_element(),
            LedgerLine::Run(run) => self.render_ledger_run(line, run, cx),
        }
    }

    fn render_ledger_run(
        &self,
        line: usize,
        run: &LedgerRun,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = theme(cx);
        let selected = self.session.log.as_ref() == Some(&run.key);
        let key = run.key.clone();
        let task = run.task.clone();
        let what = match run.steps.as_slice() {
            [step] => single(step, &t),
            steps => chain(steps, self.list_wide, &t),
        };
        let title_color = if run.deleted {
            t.faint
        } else if run.closed {
            t.body
        } else {
            t.ink
        };
        let first = &run.steps[0];
        div()
            .px(px(8.))
            .child(
                div()
                    .id(("log", line))
                    .role(Role::ListItem)
                    .aria_label(run.title.clone())
                    .aria_selected(selected)
                    .h(px(LEDGER_ROW_HEIGHT))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .rounded(px(6.))
                    .when(selected, |row| {
                        row.bg(t.tint).shadow(vec![t.inset_ring(t.tint_line)])
                    })
                    .when(!selected, |row| row.hover(|style| style.bg(t.panel)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.session.log = Some(key.clone());
                        this.open_log_task(task.clone(), window, cx);
                    }))
                    .child(
                        div()
                            .flex_none()
                            .w(px(CLOCK_WIDTH))
                            .font_family(MONO)
                            .text_size(px(11.))
                            .text_color(t.faint)
                            .child(run.clock.clone()),
                    )
                    .child(mark(first.mark, 14., &t))
                    .when(!run.reference.is_empty(), |row| {
                        row.child(ref_label(&run.reference, &run.project_key, None, &t))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .truncate()
                            .text_color(title_color)
                            .when(run.deleted, |title| title.line_through())
                            .child(run.title.clone()),
                    )
                    .child(what),
            )
            .into_any_element()
    }

    /// Goes to a ledger row's task in the view that lists it.
    pub(crate) fn open_log_task(
        &mut self,
        task: Option<aven_core::ids::TaskId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(task) = task {
            self.session.goto(task);
            self.load_selection(window, cx);
        }
        cx.notify();
    }
}

fn heading(height: f32, t: &Theme) -> Div {
    div()
        .h(px(height))
        .px(px(16.))
        .pb(px(4.))
        .flex()
        .items_end()
        .gap(px(8.))
        .text_size(px(12.))
        .line_height(px(16.))
        .text_color(t.muted)
}

fn aside(text: String, t: &Theme) -> Div {
    div()
        .font_weight(FontWeight::NORMAL)
        .text_color(t.faint)
        .child(text)
}

fn plural(count: usize, one: &str) -> String {
    format!("{count} {one}{}", if count == 1 { "" } else { "s" })
}

fn mark(mark: StepMark, size: f32, t: &Theme) -> AnyElement {
    let glyph = |path: &'static str, color| {
        icon(path, color)
            .size(px(size))
            .flex_none()
            .into_any_element()
    };
    match mark {
        StepMark::Status(status) => status_glyph_sized(status, size, t).into_any_element(),
        StepMark::Priority(priority) => priority_icon(priority, t).into_any_element(),
        StepMark::Created => glyph("icons/ra-create.svg", t.accent),
        StepMark::Icon(path) => glyph(path, t.muted),
    }
}

fn what(t: &Theme) -> Div {
    div()
        .flex_none()
        .max_w(px(380.))
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(px(4.))
        .text_size(px(12.))
        .text_color(t.muted)
        .whitespace_nowrap()
}

fn single(step: &LedgerStep, t: &Theme) -> Div {
    let tone = step.tone.map(|status| status_color(status, t));
    what(t)
        .child(div().flex_none().child(step.lead.clone()))
        .when_some(step.value.clone(), |what, value| {
            what.child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .when_some(tone, |value, tone| value.text_color(tone))
                    .child(value),
            )
        })
        .when_some(step.chip.clone(), |what, chip| {
            what.child(chip_label(chip, t))
        })
}

fn chip_label(text: String, t: &Theme) -> Div {
    div()
        .min_w_0()
        .max_w(px(220.))
        .h(px(18.))
        .px(px(6.))
        .rounded(px(9.))
        .bg(t.raised)
        .text_color(t.body)
        .line_height(px(18.))
        .truncate()
        .child(text)
}

/// A run's steps, oldest first, the words dropped when the list is narrow.
fn chain(steps: &[LedgerStep], words: bool, t: &Theme) -> Div {
    let earlier = steps.len().saturating_sub(RUN_STEPS);
    let sep = || div().flex_none().text_color(t.faint).child("›");
    let mut row = what(t).gap(px(3.));
    if earlier > 0 {
        let text = if words {
            format!("{earlier} earlier")
        } else {
            format!("+{earlier}")
        };
        row = row.child(div().flex_none().text_color(t.faint).child(text));
    }
    for (ix, step) in steps[..steps.len() - earlier].iter().rev().enumerate() {
        if ix > 0 || earlier > 0 {
            row = row.child(sep());
        }
        let tone = step.tone.map(|status| status_color(status, t));
        row = row.child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(mark(step.mark, 12., t))
                .when(words, |step_el| {
                    step_el.child(
                        div()
                            .when_some(tone, |word, tone| {
                                word.font_weight(FontWeight::MEDIUM).text_color(tone)
                            })
                            .child(step.short.clone()),
                    )
                }),
        );
    }
    row
}
