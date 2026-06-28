pub(super) fn truncate_width(value: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if UnicodeWidthStr::width(value) <= max_width {
        return value.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }
    let mut truncated = String::new();
    let mut width = 0;
    for ch in value.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width - 1 {
            break;
        }
        truncated.push(ch);
        width += ch_width;
    }
    truncated.push('…');
    truncated
}

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

    #[test]
    fn truncate_width_uses_display_width() {
        assert_eq!(truncate_width("abcdef", 4), "abc…");
        assert_eq!(truncate_width("漢字漢字", 5), "漢字…");
    }

    #[test]
    fn truncate_width_preserves_short_values() {
        assert_eq!(truncate_width("abc", 5), "abc");
    }

    #[test]
    fn truncate_width_returns_empty_for_zero_width() {
        assert_eq!(truncate_width("abc", 0), "");
    }

    #[test]
    fn truncate_width_uses_single_ellipsis_for_width_one() {
        assert_eq!(truncate_width("abc", 1), "…");
    }

    #[test]
    fn truncate_width_with_cjk() {
        // CJK characters have width 2
        assert_eq!(truncate_width("aあb", 3), "a…");
        assert_eq!(truncate_width("あいう", 4), "あ…");
    }
}
