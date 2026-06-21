use std::io::{self, IsTerminal};

use anyhow::Result;
use crossterm::style::{Color, Stylize};
use sqlx::SqliteConnection;

pub(super) async fn workspace_counts(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<(i64, i64)> {
    let active =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND deleted = 0")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?;
    let all = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok((active, all))
}

pub(super) fn sync_server_url_is_valid(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

pub(super) struct DoctorReport {
    sections: Vec<DoctorSection>,
}

impl DoctorReport {
    pub(super) fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub(super) fn section(&mut self, title: &'static str) -> &mut DoctorSection {
        self.sections.push(DoctorSection {
            title,
            rows: Vec::new(),
        });
        self.sections.last_mut().expect("section was pushed")
    }
}

pub(super) struct DoctorSection {
    title: &'static str,
    rows: Vec<DoctorRow>,
}

impl DoctorSection {
    pub(super) fn check(&mut self, label: &'static str, ok: bool, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: if ok {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Error
            },
            label,
            value: value.into(),
        });
    }

    pub(super) fn info(&mut self, label: &'static str, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: DoctorStatus::Info,
            label,
            value: value.into(),
        });
    }
}

struct DoctorRow {
    status: DoctorStatus,
    label: &'static str,
    value: String,
}

#[derive(Clone, Copy)]
enum DoctorStatus {
    Ok,
    Error,
    Info,
}

pub(super) struct DoctorRenderer {
    styled: bool,
}

impl DoctorRenderer {
    pub(super) fn auto() -> Self {
        Self {
            styled: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
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
        if self.styled {
            self.print_styled_row(row, label_width);
        } else {
            let marker = row.status.marker();
            println!("  {marker} {:<18} {}", row.label, row.value);
        }
    }

    fn print_styled_row(&self, row: &DoctorRow, label_width: usize) {
        let label = format!("{:<label_width$}", row.label);
        println!(
            "  {} {}  {}",
            row.status.icon().with(row.status.color()).bold(),
            label.with(row.status.label_color()),
            row.value.as_str().with(Color::Rgb {
                r: 150,
                g: 150,
                b: 150,
            })
        );
    }
}

impl DoctorStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "!!",
            Self::Info => "..",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Error => "✗",
            Self::Info => "·",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => Color::Green,
            Self::Error => Color::Red,
            Self::Info => Color::DarkGrey,
        }
    }

    fn label_color(self) -> Color {
        match self {
            Self::Ok | Self::Error => Color::White,
            Self::Info => Color::Grey,
        }
    }
}
