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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_or_placeholder_uses_empty_state_copy() {
        assert_eq!(description_or_placeholder(""), "(no description)");
        assert_eq!(description_or_placeholder("Body"), "Body");
    }

    #[test]
    fn labels_display_uses_none_for_empty_labels() {
        assert_eq!(labels_display(&[], ", "), "none");
        assert_eq!(
            labels_display(&["bug".to_string(), "mobile".to_string()], ", "),
            "bug, mobile"
        );
    }
}
