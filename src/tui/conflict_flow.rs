use crate::tui::text::truncate_width;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictResolutionChoice {
    Local,
    Remote,
}

pub(crate) fn truncate_value_preview(value: &str, max_width: usize) -> String {
    truncate_width(value, max_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_value_preview_uses_cell_width() {
        assert_eq!(truncate_value_preview("abc", 5), "abc");
        assert_eq!(truncate_value_preview("abcdef", 4), "abc…");
        assert_eq!(truncate_value_preview("한글입력", 5), "한글…");
        assert_eq!(truncate_value_preview("한글", 0), "");
    }
}
