use similar::TextDiff;

pub(crate) fn quote(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) fn print_multiline_block(label: &str, value: &str) {
    println!("{label}<<EOF");
    print!("{}", value);
    if !value.ends_with('\n') {
        println!();
    }
    println!("EOF");
}

pub(crate) fn print_near_error(kind: &str, input: &str, choices: &[String]) {
    eprintln!("error unknown-{kind} input={}", input);
    for choice in choices {
        eprintln!("choice {choice}");
    }
    eprintln!("hint \"retry with an exact {kind} or create it explicitly\"");
}

pub(crate) fn changed_text(changed: bool) -> &'static str {
    if changed { "yes" } else { "none" }
}

pub(crate) fn print_text_diff(from_label: &str, old: &str, to_label: &str, new: &str) {
    let diff = TextDiff::from_lines(old, new);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(from_label, to_label)
        .to_string();
    if unified.is_empty() {
        println!(" no changes");
    } else {
        print!("{unified}");
    }
}
