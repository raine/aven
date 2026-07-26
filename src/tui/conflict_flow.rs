#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictResolutionChoice {
    Local,
    Remote,
}

pub(crate) fn truncate_value_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_value_preview_uses_character_count() {
        assert_eq!(truncate_value_preview("abc", 5), "abc");
        assert_eq!(truncate_value_preview("abcdef", 3), "abc…");
    }
}
