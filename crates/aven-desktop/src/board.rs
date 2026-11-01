//! The columns layout: the view's groups side by side, with cards to drag.
use accesskit::Role;
use aven_core::choices::TaskStatus;
use aven_core::ids::TaskId;
use gpui::{
    Context, FontWeight, MouseButton, MouseDownEvent, Pixels, Point, Render, Window, div,
    prelude::*, px,
};

use crate::adapter::{Group, TaskRow};
use crate::kit::*;
use crate::shell::Root;
use crate::theme::{Theme, theme};

const COLUMN_BASIS: f32 = 200.;

/// A card on its way to another column.
#[derive(Clone)]
struct CardDrag {
    id: TaskId,
    title: String,
    position: Point<Pixels>,
}

impl Render for CardDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme(cx);
        div().pl(self.position.x).pt(self.position.y).child(
            div()
                .w(px(200.))
                .px(px(10.))
                .py(px(9.))
                .rounded(px(7.))
                .bg(t.page)
                .shadow(t.menu_shadow())
                .text_color(t.ink)
                .truncate()
                .child(self.title.clone()),
        )
    }
}

impl Root {
    fn card(&self, row: &TaskRow, cx: &mut Context<Self>, t: &Theme) -> impl IntoElement + use<> {
        let selected = self.session.selected.as_ref() == Some(&row.id);
        let (id, menu_id) = (row.id.clone(), row.id.clone());
        let drag = CardDrag {
            id: row.id.clone(),
            title: row.title.clone(),
            position: Point::default(),
        };
        let ring = if selected { t.accent } else { t.line };
        div()
            .id(gpui::ElementId::Name(format!("card-{}", row.id).into()))
            .role(Role::ListItem)
            .aria_label(format!("{} {}", row.reference, row.title))
            .aria_selected(selected)
            .flex_none()
            .px(px(10.))
            .py(px(9.))
            .rounded(px(7.))
            .bg(t.page)
            .shadow(vec![t.ring(ring)])
            .on_drag(drag, |drag, offset, _, cx| {
                let mut drag = drag.clone();
                drag.position = offset;
                cx.new(|_| drag)
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.select(id.clone(), window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.row_menu(menu_id.clone(), event.position, window, cx);
                }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .child(status_glyph(row.status, t))
                    .child(ref_label(&row.reference, &row.project, None, t))
                    .child(div().flex_1())
                    .child(priority_icon(row.priority, t)),
            )
            .child(
                div()
                    .mt(px(4.))
                    .text_color(if row.status == TaskStatus::Done {
                        t.faint
                    } else {
                        t.ink
                    })
                    .child(row.title.clone()),
            )
    }

    fn column(
        &self,
        index: usize,
        group: &Group,
        cx: &mut Context<Self>,
        t: &Theme,
    ) -> impl IntoElement + use<> {
        let lane = group.lane;
        let over = t.ring(t.accent);
        let mut cards = div()
            .id(("column-cards", index))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(6.))
            .flex()
            .flex_col()
            .gap(px(6.));
        for task in &group.rows {
            cards = cards.child(self.card(&self.session.snapshot.tasks[*task], cx, t));
        }
        div()
            .id(("column", index))
            .role(Role::List)
            .aria_label(group.name.clone())
            .flex_1()
            .min_w(px(COLUMN_BASIS))
            .h_full()
            .flex()
            .flex_col()
            .rounded(px(8.))
            .bg(t.panel)
            .shadow(vec![t.inset_ring(t.line)])
            .when(lane.is_some(), |column| {
                column
                    .drag_over::<CardDrag>(move |style, _, _, _| style.shadow(vec![over.clone()]))
                    .on_drop(cx.listener(move |this, drag: &CardDrag, _, cx| {
                        if let Some(lane) = lane {
                            this.drop_on_lane(&drag.id, lane, cx);
                        }
                    }))
            })
            .child(
                div()
                    .flex_none()
                    .h(px(36.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.muted)
                    .child(group.name.clone())
                    .child(
                        div()
                            .text_color(t.faint)
                            .font_weight(FontWeight::NORMAL)
                            .child(group.rows.len().to_string()),
                    ),
            )
            .child(cards)
    }

    pub(crate) fn render_board(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = theme(cx);
        let mut board = div()
            .id("board")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .p(px(12.))
            .flex()
            .gap(px(10.))
            .overflow_x_scroll();
        for (index, group) in self.session.snapshot.groups.iter().enumerate() {
            board = board.child(self.column(index, group, cx, &t));
        }
        board
    }
}
