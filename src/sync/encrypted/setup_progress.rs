use std::io::{IsTerminal, Write};

use super::{Progress, Stage};
use crate::sync::progress_text::amount_text;

const UPLOADING: &str = "Uploading encrypted data...";

/// Presentation only: failed writes never change the setup result.
pub(super) struct SetupProgress<W: Write> {
    writer: W,
    interactive: bool,
    line: Option<String>,
}

impl SetupProgress<std::io::Stderr> {
    pub(super) fn stderr() -> Self {
        Self::new(
            std::io::stderr(),
            std::io::stderr().is_terminal()
                && std::env::var_os("TERM").is_none_or(|term| term != "dumb"),
        )
    }

    pub(super) fn update(&mut self, progress: Progress) {
        let columns = if self.interactive {
            crossterm::terminal::size().ok().map(|(columns, _)| columns)
        } else {
            None
        };
        self.render(progress, columns);
    }
}

impl<W: Write> SetupProgress<W> {
    fn new(writer: W, interactive: bool) -> Self {
        Self {
            writer,
            interactive,
            line: None,
        }
    }

    fn render(&mut self, progress: Progress, columns: Option<u16>) {
        if !self.interactive {
            if progress == Stage::UploadingData.into() {
                let _ = writeln!(self.writer, "{UPLOADING}");
            }
            return;
        }
        let text = match progress.stage {
            Stage::UploadingData => match progress.amount {
                Some(amount) => {
                    let amount = amount_text(amount);
                    let full = format!("{UPLOADING} {amount}");
                    if columns.is_some_and(|columns| full.chars().count() >= usize::from(columns)) {
                        format!("Uploading · {amount}")
                    } else {
                        full
                    }
                }
                None => UPLOADING.to_string(),
            },
            Stage::FinishingSetup => "Finishing setup...".to_string(),
            _ => {
                self.finish();
                return;
            }
        };
        // Leave the last column unused to avoid wrapping the active line.
        let limit = usize::from(columns.unwrap_or(80).saturating_sub(1));
        let text: String = text.chars().take(limit).collect();
        if self.line.as_ref() == Some(&text) {
            return;
        }
        let _ = write!(self.writer, "\r\x1b[2K{text}");
        self.line = Some(text);
        let _ = self.writer.flush();
    }

    fn finish(&mut self) {
        if self.line.take().is_some() {
            let _ = writeln!(self.writer);
            let _ = self.writer.flush();
        }
    }
}

impl<W: Write> Drop for SetupProgress<W> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::super::Amount;
    use super::*;

    fn bytes(done: u64, total: Option<u64>) -> Progress {
        Progress {
            stage: Stage::UploadingData,
            amount: Some(Amount::Bytes { done, total }),
        }
    }

    #[test]
    fn terminal_updates_and_finishes_on_its_own_line() {
        let mut output = Vec::new();
        {
            let mut printer = SetupProgress::new(&mut output, true);
            printer.render(Stage::UploadingData.into(), Some(80));
            printer.render(bytes(0, Some(4096)), Some(80));
            printer.render(bytes(1024, Some(4096)), Some(80));
            let writes = printer.writer.len();
            printer.render(bytes(1024, Some(4096)), Some(80));
            assert_eq!(printer.writer.len(), writes);
            printer.render(bytes(4096, Some(4096)), Some(80));
            printer.render(Stage::FinishingSetup.into(), Some(80));
        }
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("1.0 KiB of 4.0 KiB · 25%"), "{output:?}");
        assert!(output.contains("4.0 KiB of 4.0 KiB · 100%"), "{output:?}");
        assert!(
            output.ends_with("\r\x1b[2KFinishing setup...\n"),
            "{output:?}"
        );
    }

    #[test]
    fn redirected_output_preserves_the_single_announcement() {
        let mut output = Vec::new();
        {
            let mut printer = SetupProgress::new(&mut output, false);
            printer.render(Stage::UploadingData.into(), None);
            printer.render(bytes(0, Some(4)), None);
            printer.render(bytes(4, Some(4)), None);
            printer.render(Stage::FinishingSetup.into(), None);
        }
        assert_eq!(output, b"Uploading encrypted data...\n");
    }

    #[test]
    fn interrupted_upload_is_closed_before_error_output() {
        let mut output = Vec::new();
        {
            let mut printer = SetupProgress::new(&mut output, true);
            printer.render(bytes(2, None), None);
        }
        writeln!(output, "Error: upload failed").unwrap();
        assert!(
            String::from_utf8(output)
                .unwrap()
                .ends_with("2 B so far\nError: upload failed\n")
        );
    }

    #[test]
    fn narrow_terminal_keeps_the_frame_on_one_line() {
        let mut output = Vec::new();
        {
            let mut printer = SetupProgress::new(&mut output, true);
            printer.render(bytes(1024, Some(4096)), Some(40));
        }
        let output = String::from_utf8(output).unwrap();
        let frame = output.strip_prefix("\r\x1b[2K").unwrap().trim_end();
        assert!(frame.chars().count() < 40);
        assert!(frame.contains("25%"), "{frame}");
    }

    #[test]
    fn failed_output_does_not_panic() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
        }
        let mut printer = SetupProgress::new(Broken, true);
        printer.render(bytes(0, Some(4)), None);
        printer.render(bytes(4, Some(4)), None);
    }
}
