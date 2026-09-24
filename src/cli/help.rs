use std::fmt::Write as _;

use clap::builder::styling::{AnsiColor, Effects, Style, Styles};
use clap::{CommandFactory, FromArgMatches};

use super::Cli;

const ACCENT_STYLE: Style = AnsiColor::Magenta.on_default();

const HEADING_STYLE: Style = AnsiColor::Magenta.on_default().effects(Effects::BOLD);

const LITERAL_STYLE: Style = Style::new();

const PLACEHOLDER_STYLE: Style = Style::new().effects(Effects::DIMMED);

const DESCRIPTION_STYLE: Style = Style::new();

pub(super) const STYLES: Styles = Styles::styled()
    .header(HEADING_STYLE)
    .usage(HEADING_STYLE)
    .literal(LITERAL_STYLE)
    .placeholder(PLACEHOLDER_STYLE)
    .context(DESCRIPTION_STYLE)
    .context_value(AnsiColor::Yellow.on_default())
    .valid(AnsiColor::Green.on_default())
    .invalid(AnsiColor::Red.on_default().effects(Effects::BOLD))
    .error(AnsiColor::Red.on_default().effects(Effects::BOLD));

pub(super) const HELP_SECTIONS: &[HelpSection] = &[
    HelpSection {
        heading: "TASKS",
        commands: &[
            "add",
            "list",
            "search",
            "context",
            "show",
            "edit",
            "note",
            "note-delete",
            "dep",
            "related",
            "epic",
            "text",
            "bulk-update",
            "delete",
            "restore",
            "recur",
        ],
    },
    HelpSection {
        heading: "WORKSPACE",
        commands: &["workspace", "project", "label", "metadata"],
    },
    HelpSection {
        heading: "SYNC",
        commands: &["sync", "server", "conflict", "daemon"],
    },
    HelpSection {
        heading: "INTERACTIVE",
        commands: &["tui", "demo"],
    },
    HelpSection {
        heading: "AGENTS",
        commands: &["prime", "skill"],
    },
    HelpSection {
        heading: "SETUP",
        commands: &["config", "doctor", "update"],
    },
    HelpSection {
        heading: "ATTACHMENTS",
        commands: &["attachment"],
    },
    HelpSection {
        heading: "DATA SAFETY",
        commands: &["backup", "export", "import"],
    },
];

pub(super) struct HelpSection {
    pub(super) heading: &'static str,
    pub(super) commands: &'static [&'static str],
}

pub(crate) fn parse_from<I, T>(args: I) -> Cli
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let mut command = Cli::command();
    let help = render_top_level_help(&command);
    command = command.override_help(help);
    let matches = command.get_matches_from(args);
    Cli::from_arg_matches(&matches).expect("clap validates matches")
}

fn render_top_level_help(command: &clap::Command) -> String {
    let mut help = String::new();
    writeln!(&mut help, "Local-first task manager").unwrap();
    writeln!(&mut help).unwrap();
    writeln!(
        &mut help,
        "{} aven {} {}",
        paint_heading("USAGE:"),
        paint("[OPTIONS]", LITERAL_STYLE),
        paint("[COMMAND]", PLACEHOLDER_STYLE)
    )
    .unwrap();
    writeln!(&mut help).unwrap();

    for section in HELP_SECTIONS {
        render_section(&mut help, command, section);
    }

    render_help_section(&mut help);
    render_options_section(&mut help);
    help
}

fn render_section(help: &mut String, command: &clap::Command, section: &HelpSection) {
    writeln!(help, "{}", paint_heading(section.heading)).unwrap();
    let width = help_row_width(section.commands.iter().copied());
    for name in section.commands {
        let about = command_about(command, name).unwrap_or_default();
        render_row(help, name, &paint(name, LITERAL_STYLE), &about, width);
    }
    writeln!(help).unwrap();
}

fn render_help_section(help: &mut String) {
    writeln!(help, "{}", paint_heading("HELP")).unwrap();
    render_row(
        help,
        "help",
        &paint("help", LITERAL_STYLE),
        "Print this message or the help of the given subcommand(s)",
        help_row_width(["help"]),
    );
    writeln!(help).unwrap();
}

fn render_options_section(help: &mut String) {
    writeln!(help, "{}", paint_heading("OPTIONS")).unwrap();
    let width = help_row_width([
        "--db <DB>",
        "--workspace <WORKSPACE>",
        "-V, --version",
        "-h, --help",
    ]);
    render_row(
        help,
        "--db <DB>",
        &format!(
            "{} {}",
            paint("--db", LITERAL_STYLE),
            paint("<DB>", PLACEHOLDER_STYLE)
        ),
        "Use a specific SQLite database path",
        width,
    );
    render_row(
        help,
        "--workspace <WORKSPACE>",
        &format!(
            "{} {}",
            paint("--workspace", LITERAL_STYLE),
            paint("<WORKSPACE>", PLACEHOLDER_STYLE)
        ),
        "Use a specific workspace by name or key",
        width,
    );
    render_row(
        help,
        "-V, --version",
        &format!(
            "{}, {}",
            paint("-V", LITERAL_STYLE),
            paint("--version", LITERAL_STYLE)
        ),
        "Print version",
        width,
    );
    render_row(
        help,
        "-h, --help",
        &format!(
            "{}, {}",
            paint("-h", LITERAL_STYLE),
            paint("--help", LITERAL_STYLE)
        ),
        "Print help",
        width,
    );
}

fn command_about(command: &clap::Command, name: &str) -> Option<String> {
    command
        .get_subcommands()
        .find(|subcommand| subcommand.get_name() == name)
        .and_then(|subcommand| subcommand.get_about())
        .map(|about| about.to_string())
}

pub(super) fn help_row_width<'a>(names: impl IntoIterator<Item = &'a str>) -> usize {
    names
        .into_iter()
        .map(str::len)
        .max()
        .unwrap_or_default()
        .saturating_add(2)
}

pub(super) fn render_row(
    help: &mut String,
    plain_name: &str,
    styled_name: &str,
    description: &str,
    width: usize,
) {
    write!(help, "  {styled_name}").unwrap();
    for _ in plain_name.len()..width {
        help.push(' ');
    }
    writeln!(help, "{}", paint(description, DESCRIPTION_STYLE)).unwrap();
}

fn paint_heading(text: &str) -> String {
    format!(
        "{} {}",
        paint("›", ACCENT_STYLE),
        paint(text, HEADING_STYLE)
    )
}

fn paint(text: &str, style: Style) -> String {
    format!("{}{}{}", style.render(), text, style.render_reset())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_rows_align_to_each_section_longest_command() {
        let commands = ["add", "command-name-that-exceeds-fixed-width"];
        let width = help_row_width(commands);
        let mut rendered = String::new();

        for command in commands {
            render_row(&mut rendered, command, command, "description", width);
        }

        let rows = rendered.lines().collect::<Vec<_>>();
        let description_columns = rows
            .iter()
            .map(|row| row.find("description").unwrap())
            .collect::<Vec<_>>();
        assert_eq!(description_columns[0], description_columns[1]);
        assert!(rows[1].contains("command-name-that-exceeds-fixed-width  description"));
    }
}
