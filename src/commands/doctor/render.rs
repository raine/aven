use std::io::{self, IsTerminal};

use crossterm::style::{Color, Stylize};

use super::{DoctorReport, DoctorRow, DoctorStatus};

pub(in crate::commands) struct DoctorRenderer {
    styled: bool,
}

impl DoctorRenderer {
    pub(super) fn auto() -> Self {
        Self {
            styled: io::stdout().is_terminal()
                && std::env::var_os("NO_COLOR").is_none()
                && std::env::var_os("TERM").is_none_or(|term| term != "dumb"),
        }
    }

    pub(super) fn print(&self, report: &DoctorReport) {
        if self.styled {
            println!(
                "{}",
                "aven doctor"
                    .with(Color::Rgb {
                        r: 45,
                        g: 174,
                        b: 135
                    })
                    .bold()
            );
        } else {
            println!("aven doctor");
        }
        for section in &report.sections {
            println!();
            self.print_section(section.title);
            let label_width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or(0);
            for row in &section.rows {
                self.print_row(row, label_width);
            }
        }
        println!();
        println!("overall: {}", report.overall_status.as_str());
    }

    fn print_section(&self, title: &str) {
        if self.styled {
            println!("{}", title.with(Color::Cyan).bold());
        } else {
            println!("{title}");
            println!("{}", "-".repeat(title.len()));
        }
    }

    fn print_row(&self, row: &DoctorRow, label_width: usize) {
        let value = row
            .skipped_reason
            .as_ref()
            .map(|reason| format!("skipped: {reason}"))
            .unwrap_or_else(|| row.value.clone());
        let value = crate::render::inline_code(&value, self.styled);
        if self.styled {
            let label = format!("{:<label_width$}", row.label);
            println!(
                "  {} {}  {}",
                row.status.icon().with(row.status.color()).bold(),
                label.with(row.status.label_color()),
                value.with(Color::Rgb {
                    r: 150,
                    g: 150,
                    b: 150,
                })
            );
        } else {
            println!("  {} {:<18} {value}", row.status.marker(), row.label);
        }
    }
}

impl DoctorStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Skipped => "skipped",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "!!",
            Self::Error => "!!",
            Self::Info => "..",
            Self::Skipped => "--",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warning => "!",
            Self::Error => "✗",
            Self::Info => "·",
            Self::Skipped => "-",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
            Self::Info | Self::Skipped => Color::DarkGrey,
        }
    }

    fn label_color(self) -> Color {
        match self {
            Self::Ok | Self::Warning | Self::Error => Color::White,
            Self::Info | Self::Skipped => Color::Grey,
        }
    }
}
