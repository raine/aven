pub(super) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    if max_chars == 0 {
        return String::new();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let mut truncated = value.chars().take(max_chars - 1).collect::<String>();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_preserves_short_values() {
        assert_eq!(truncate_chars("abc", 5), "abc");
    }

    #[test]
    fn truncate_chars_returns_empty_for_zero_width() {
        assert_eq!(truncate_chars("abc", 0), "");
    }

    #[test]
    fn truncate_chars_uses_single_ellipsis_for_width_one() {
        assert_eq!(truncate_chars("abc", 1), "…");
    }

    #[test]
    fn truncate_chars_uses_character_count() {
        assert_eq!(truncate_chars("abcdef", 4), "abc…");
        assert_eq!(truncate_chars("éclair", 4), "écl…");
    }
}
