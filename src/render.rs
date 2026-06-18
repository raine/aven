pub(crate) fn quote(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) fn print_near_error(kind: &str, input: &str, choices: &[String]) {
    eprintln!("error unknown-{kind} input={}", input);
    for choice in choices {
        eprintln!("choice {choice}");
    }
    eprintln!("hint \"retry with an exact {kind} or create it explicitly\"");
}
