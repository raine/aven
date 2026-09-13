use ratatui::layout::{Constraint, Layout, Rect};

use crate::config::TableColumn;

/// Frame-local content geometry indexed by semantic identity, not display position.
#[derive(Debug, Clone, Copy)]
pub(super) struct TableLayout {
    cells: [Rect; 9],
    state_column: Option<TableColumn>,
    state_gutter: Rect,
}

impl TableLayout {
    const STATE_GUTTER_WIDTH: u16 = 3;

    pub(super) fn resolve(columns: &[Constraint; 9], order: &[TableColumn], width: u16) -> Self {
        // Zero-width semantic columns are omitted before adding gutters. Fixed widths include
        // their default gutters, while content width stays semantic.
        let cells = Self::resolve_cells(columns, order, width, 0);
        let state_column = Self::inline_state_column(&cells, order);
        if state_column.is_some() {
            return Self {
                cells,
                state_column,
                state_gutter: Rect::default(),
            };
        }

        // A table without a usable ref or title cell still needs a visible row state surface.
        // Keep it separate from the configured content so fallback columns retain their width.
        let state_gutter_width = width.min(Self::STATE_GUTTER_WIDTH);
        Self {
            cells: Self::resolve_cells(
                columns,
                order,
                width.saturating_sub(state_gutter_width),
                state_gutter_width,
            ),
            state_column: None,
            state_gutter: Rect::new(0, 0, state_gutter_width, 1),
        }
    }

    fn resolve_cells(
        columns: &[Constraint; 9],
        order: &[TableColumn],
        width: u16,
        offset: u16,
    ) -> [Rect; 9] {
        let visible = order
            .iter()
            .copied()
            .filter(|column| !matches!(columns[*column as usize], Constraint::Length(0)))
            .collect::<Vec<_>>();
        let constraints = visible.iter().enumerate().map(|(position, column)| {
            let constraint = columns[*column as usize];
            match constraint {
                Constraint::Length(width) if width > 0 => {
                    let content_width = if *column == TableColumn::Time {
                        width
                    } else {
                        width.saturating_sub(1)
                    };
                    Constraint::Length(content_width + u16::from(position + 1 < visible.len()))
                }
                other => other,
            }
        });
        let areas = Layout::horizontal(constraints).split(Rect::new(offset, 0, width, 1));
        let mut cells = [Rect::default(); 9];
        for (position, (column, area)) in visible.iter().zip(areas.iter()).enumerate() {
            let mut area = *area;
            if position + 1 < visible.len() {
                area.width = area.width.saturating_sub(1);
            }
            cells[*column as usize] = area;
        }
        cells
    }

    fn inline_state_column(cells: &[Rect; 9], order: &[TableColumn]) -> Option<TableColumn> {
        if cells[TableColumn::Ref as usize].width >= Self::STATE_GUTTER_WIDTH
            && order.contains(&TableColumn::Ref)
        {
            return Some(TableColumn::Ref);
        }
        if cells[TableColumn::Title as usize].width >= Self::STATE_GUTTER_WIDTH
            && order.contains(&TableColumn::Title)
        {
            return Some(TableColumn::Title);
        }
        None
    }

    pub(super) fn state_column(self) -> Option<TableColumn> {
        self.state_column
    }

    pub(super) fn state_gutter(self, row: Rect) -> Rect {
        Rect::new(
            row.x.saturating_add(self.state_gutter.x),
            row.y,
            self.state_gutter.width,
            row.height,
        )
    }

    pub(super) fn cell(self, column: TableColumn, row: Rect) -> Rect {
        let area = self.cells[column as usize];
        Rect::new(row.x.saturating_add(area.x), row.y, area.width, row.height)
    }

    pub(super) fn widths(self) -> [usize; 9] {
        self.cells.map(|area| area.width as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reordering_preserves_fixed_content_widths_and_collapsed_columns() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(0),
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(0),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        let original = TableLayout::resolve(&columns, &TableColumn::ALL, 120).widths();
        for rotation in 0..TableColumn::ALL.len() {
            let mut order = TableColumn::ALL;
            order.rotate_left(rotation);
            let widths = TableLayout::resolve(&columns, &order, 120).widths();
            for column in TableColumn::ALL {
                if column != TableColumn::Title {
                    assert_eq!(widths[column as usize], original[column as usize]);
                }
            }
        }
    }

    #[test]
    fn hidden_columns_have_no_cells_or_gutters() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        let layout = TableLayout::resolve(
            &columns,
            &[TableColumn::Ref, TableColumn::Status, TableColumn::Time],
            40,
        );
        assert_eq!(
            layout.cell(TableColumn::Ref, Rect::new(0, 0, 40, 1)),
            Rect::new(0, 0, 11, 1)
        );
        assert_eq!(
            layout.cell(TableColumn::Status, Rect::new(0, 0, 40, 1)),
            Rect::new(12, 0, 9, 1)
        );
        assert_eq!(
            layout.cell(TableColumn::Time, Rect::new(0, 0, 40, 1)),
            Rect::new(22, 0, 5, 1)
        );
        for column in [
            TableColumn::Title,
            TableColumn::Labels,
            TableColumn::Metadata,
            TableColumn::Project,
            TableColumn::Priority,
        ] {
            assert_eq!(layout.cell(column, Rect::new(0, 0, 40, 1)).width, 0);
        }
    }

    #[test]
    fn hidden_status_has_no_state_column_or_hit_area() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        let layout = TableLayout::resolve(&columns, &[TableColumn::Title, TableColumn::Time], 40);
        assert_eq!(layout.state_column(), Some(TableColumn::Title));
        assert_eq!(
            layout
                .cell(TableColumn::Status, Rect::new(7, 3, 40, 1))
                .width,
            0
        );
    }

    #[test]
    fn fallback_state_gutter_preserves_single_column_content_width() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(12),
            Constraint::Length(6),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        for (column, content_width) in [
            (TableColumn::Status, 9),
            (TableColumn::Priority, 2),
            (TableColumn::Time, 5),
        ] {
            let layout = TableLayout::resolve(&columns, &[column], 40);
            let row = Rect::new(7, 3, 40, 1);
            assert_eq!(layout.state_column(), None);
            assert_eq!(layout.state_gutter(row), Rect::new(7, 3, 3, 1));
            assert_eq!(layout.cell(column, row), Rect::new(10, 3, content_width, 1));
        }
    }

    #[test]
    fn all_zero_width_fallback_still_has_a_state_gutter() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(0),
            Constraint::Length(0),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(0),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        for column in [
            TableColumn::Labels,
            TableColumn::Metadata,
            TableColumn::Priority,
        ] {
            let layout = TableLayout::resolve(&columns, &[column], 24);
            assert_eq!(layout.state_column(), None);
            assert_eq!(layout.state_gutter(Rect::new(0, 1, 24, 1)).width, 3);
            assert_eq!(layout.cell(column, Rect::new(0, 1, 24, 1)).width, 0);
        }
    }

    #[test]
    fn default_geometry_preserves_expansion_and_gutters_at_all_widths() {
        let columns = [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(0),
            Constraint::Length(3),
            Constraint::Length(9),
            Constraint::Length(10),
            Constraint::Length(0),
            Constraint::Length(5),
            Constraint::Length(0),
        ];
        for width in [40, 64, 89, 90, 120, 200] {
            let row = Rect::new(7, 3, width, 1);
            let original = Layout::horizontal(columns).areas::<9>(row);
            let layout = TableLayout::resolve(&columns, &TableColumn::ALL, width);
            let last_visible = TableColumn::ALL
                .into_iter()
                .rev()
                .find(|column| original[*column as usize].width > 0);
            for (index, column) in TableColumn::ALL.into_iter().enumerate() {
                if matches!(columns[index], Constraint::Length(0)) {
                    assert_eq!(layout.cell(column, row).width, 0);
                    continue;
                }
                let mut expected = original[index];
                if last_visible != Some(column) {
                    expected.width = expected.width.saturating_sub(1);
                }
                assert_eq!(layout.cell(column, row), expected);
            }
        }
    }
}
