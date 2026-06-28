pub(super) fn labels_display(labels: &[String], separator: &str) -> String {
    if labels.is_empty() {
        "none".to_string()
    } else {
        labels.join(separator)
    }
}

pub(super) fn description_or_placeholder(description: &str) -> String {
    if description.is_empty() {
        "(no description)".to_string()
    } else {
        description.to_string()
    }
}

pub(super) fn description_preview_text(description: &str) -> String {
    if description.is_empty() {
        return "(no description)".to_string();
    }

    let mut text = String::with_capacity(description.len());
    let mut chars = description.chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(ch, '\n' | '\r') {
            while matches!(chars.peek(), Some('\n' | '\r' | ' ' | '\t')) {
                chars.next();
            }
            if !text.is_empty() && !text.ends_with(' ') {
                text.push(' ');
            }
        } else {
            text.push(ch);
        }
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_preview_text_collapses_newlines_to_spaces() {
        assert_eq!(
            description_preview_text("First sentence.\nSecond sentence.\n\n- one\n- two"),
            "First sentence. Second sentence. - one - two"
        );
    }

    #[test]
    fn description_preview_text_trims_newline_padding() {
        assert_eq!(
            description_preview_text("\n\n  First paragraph.\n\n\tSecond paragraph.  \n"),
            "First paragraph. Second paragraph."
        );
        assert_eq!(description_preview_text(""), "(no description)");
    }
}
