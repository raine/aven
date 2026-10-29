use std::io::IsTerminal;

use anyhow::Context;
use clap::builder::styling::AnsiColor;
use similar::TextDiff;

pub fn quote(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

pub fn print_multiline_block(label: &str, value: &str) {
    println!("{label}<<EOF");
    print!("{}", value);
    if !value.ends_with('\n') {
        println!();
    }
    println!("EOF");
}

pub fn print_near_error(kind: &str, input: &str, choices: &[String]) {
    eprintln!("error unknown-{kind} input={}", input);
    for choice in choices {
        eprintln!("choice {choice}");
    }
    eprintln!("hint \"retry with an exact {kind} or create it explicitly\"");
}

pub fn changed_text(changed: bool) -> &'static str {
    if changed { "yes" } else { "none" }
}

pub fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

pub struct KvLine {
    parts: Vec<String>,
}

impl KvLine {
    pub fn new(head: impl Into<String>) -> Self {
        Self {
            parts: vec![head.into()],
        }
    }

    pub fn field(mut self, key: &str, value: impl std::fmt::Display) -> Self {
        self.parts.push(format!("{key}={value}"));
        self
    }

    pub fn quoted(mut self, key: &str, value: &str) -> Self {
        self.parts.push(format!("{key}={}", quote(value)));
        self
    }

    pub fn optional(mut self, key: &str, value: Option<String>) -> Self {
        if let Some(value) = value {
            self.parts.push(format!("{key}={value}"));
        }
        self
    }

    pub fn finish(self) -> String {
        self.parts.join(" ")
    }
}

pub fn print_json_pretty<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    serde_json::to_writer_pretty(std::io::stdout(), value).context("could not serialize JSON")?;
    println!();
    Ok(())
}

pub fn print_text_diff(from_label: &str, old: &str, to_label: &str, new: &str) {
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

pub(crate) fn format_bytes(bytes: impl Into<i128>) -> String {
    let bytes = bytes.into();
    const KIB: i128 = 1024;
    const MIB: i128 = KIB * 1024;
    const GIB: i128 = MIB * 1024;
    const TIB: i128 = GIB * 1024;
    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// Inline command suggestions use the CLI help's magenta accent.
pub(crate) fn stdout_command(command: &str) -> String {
    command_suggestion(
        command,
        command_color_enabled(std::io::stdout().is_terminal()),
    )
}

pub(crate) fn stderr_command(command: &str) -> String {
    command_suggestion(
        command,
        command_color_enabled(std::io::stderr().is_terminal()),
    )
}

/// Render paired backtick spans in application-authored terminal prose.
/// User-authored Markdown and structured output must bypass this formatter.
pub(crate) fn inline_code(text: &str, color: bool) -> String {
    let mut output = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let Some(close) = rest[open + 1..].find('`') else {
            break;
        };
        output.push_str(&rest[..open]);
        output.push_str(&command_suggestion(
            &rest[open + 1..open + 1 + close],
            color,
        ));
        rest = &rest[open + close + 2..];
    }
    output.push_str(rest);
    output
}

pub(crate) fn stdout_inline_code(text: &str) -> String {
    inline_code(text, command_color_enabled(std::io::stdout().is_terminal()))
}

pub(crate) fn stderr_inline_code(text: &str) -> String {
    inline_code(text, command_color_enabled(std::io::stderr().is_terminal()))
}

fn command_color_enabled(terminal: bool) -> bool {
    terminal
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var_os("TERM").is_none_or(|term| term != "dumb")
}

fn command_suggestion(command: &str, color: bool) -> String {
    if color {
        let style = AnsiColor::Magenta.on_default();
        format!("{}{command}{}", style.render(), style.render_reset())
    } else {
        command.to_owned()
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    #[test]
    fn inline_prose_preserves_text_and_styles_only_paired_spans() {
        let text = "Run `aven sync` or `aven doctor`. Unmatched ` stays.";
        assert_eq!(
            inline_code(text, false),
            "Run aven sync or aven doctor. Unmatched ` stays."
        );
        assert_eq!(
            inline_code(text, true),
            format!(
                "Run {} or {}. Unmatched ` stays.",
                command_suggestion("aven sync", true),
                command_suggestion("aven doctor", true),
            )
        );
        assert_eq!(
            inline_code("Use `aven sync\nsetup`.", false),
            "Use aven sync\nsetup."
        );
        assert_eq!(inline_code("No markup: 日本語", true), "No markup: 日本語");
    }

    #[test]
    fn suggestions_are_plain_without_color() {
        assert_eq!(
            command_suggestion("aven sync setup", false),
            "aven sync setup"
        );
        assert!(!command_color_enabled(false));
    }

    #[test]
    fn accent_is_limited_to_the_command() {
        assert_eq!(
            format!("Run {} again.", command_suggestion("aven sync", true)),
            "Run \x1b[35maven sync\x1b[0m again."
        );
    }
}
