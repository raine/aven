use anyhow::Context;
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

pub(crate) struct KvLine {
    parts: Vec<String>,
}

impl KvLine {
    pub(crate) fn new(head: impl Into<String>) -> Self {
        Self {
            parts: vec![head.into()],
        }
    }

    pub(crate) fn field(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        self.parts.push(format!("{key}={value}"));
        self
    }

    pub(crate) fn quoted(mut self, key: &str, value: &str) -> Self {
        self.parts.push(format!("{key}={}", quote(value)));
        self
    }

    pub(crate) fn optional(mut self, key: &str, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.parts.push(format!("{key}={value}"));
        }
        self
    }

    pub(crate) fn finish(self) -> String {
        self.parts.join(" ")
    }
}

pub(crate) fn print_json_pretty<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), value).context("could not serialize JSON")?;
    println!();
    Ok(())
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
