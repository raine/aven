use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

use crate::cli::PairArgs;
use crate::config::AppConfig;
use crate::pairing::{PairingError, PairingQr, pairing_invitation, pairing_presentation};

const QR_LINE_STYLE: &str = "\x1b[30;47m";
const STYLE_RESET: &str = "\x1b[0m";

pub(crate) fn cmd_sync_pair(config: &AppConfig, args: PairArgs) -> Result<()> {
    if args.copy {
        let invitation = pairing_invitation(config, args.server.as_deref()).map_err(|error| {
            if error == PairingError::InvitationTooLarge {
                anyhow!("error pairing-invitation-too-large hint=\"shorten sync.auth_token or the server URL\"")
            } else {
                pairing_error(error)
            }
        })?;
        let uri = invitation
            .encode()
            .map_err(|error| pairing_error(error.into()))?;
        copy_invitation(&uri)?;
        println!(
            "{}",
            render_copy_confirmation(
                invitation.server_origin(),
                io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
            )
        );
        println!("On your iPhone, tap Paste during Aven onboarding.");
        println!("The invitation contains credentials. Clipboard history may retain it.");
        println!("The invitation stops working when you rotate sync.auth_token.");
        return Ok(());
    }
    let presentation =
        pairing_presentation(config, args.server.as_deref()).map_err(pairing_error)?;
    let stdout_is_terminal = io::stdout().is_terminal();
    let (terminal_columns, styled) = output_options(
        stdout_is_terminal,
        std::env::var_os("NO_COLOR").is_some(),
        || crossterm::terminal::size().ok().map(|(columns, _)| columns),
    );
    let qr = render_terminal_qr(presentation.qr(), terminal_columns, styled)?;

    println!("Pairing server: {}", presentation.server_identity());
    print!("{qr}");
    println!("Scan this code during Aven iOS onboarding.");
    println!("The invitation stops working when you rotate sync.auth_token.");
    Ok(())
}

fn render_copy_confirmation(origin: &str, styled: bool) -> String {
    let message = format!("Pairing invitation copied for {origin}.");
    if styled {
        format!("\x1b[1;39m{message}{STYLE_RESET}")
    } else {
        message
    }
}

fn copy_invitation(uri: &str) -> Result<()> {
    if ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| !value.is_empty()))
    {
        bail!(
            "error pairing-copy-remote-session hint=\"run --copy on your local desktop, or use the QR code\""
        );
    }
    #[cfg(target_os = "macos")]
    let commands: &[(&str, &[&str])] = &[("pbcopy", &[])];
    #[cfg(target_os = "linux")]
    let commands: &[(&str, &[&str])] = &[
        ("wl-copy", &["--type", "text/plain"]),
        ("xclip", &["-selection", "clipboard"]),
    ];
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let commands: &[(&str, &[&str])] = &[];

    for (program, args) in commands {
        if write_invitation_to_clipboard(program, args, uri).is_ok() {
            return Ok(());
        }
    }
    bail!(
        "error pairing-copy-unavailable hint=\"use a local macOS clipboard or install wl-copy/xclip in a Linux desktop session; otherwise use the QR code\""
    );
}

// Invitation bytes go only to stdin. Clipboard helper output and errors may
// echo credentials, so neither is retained or included in diagnostics.
fn write_invitation_to_clipboard(program: &str, args: &[&str], uri: &str) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("clipboard stdin unavailable"))?
            .write_all(uri.as_bytes())?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(io::Error::other("clipboard copy failed"))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "clipboard copy timed out",
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn pairing_error(error: PairingError) -> anyhow::Error {
    match error {
        PairingError::MissingServer => anyhow!(
            "error pairing-server-required hint=\"pass --server or configure sync.server_url\""
        ),
        PairingError::MissingToken => anyhow!(
            "error pairing-auth-token-required hint=\"configure a nonempty sync.auth_token\""
        ),
        PairingError::LoopbackServer => anyhow!(
            "error pairing-server-not-reachable hint=\"pass --server with a URL the phone can reach\""
        ),
        PairingError::InvalidServerUrl => anyhow!(
            "error pairing-server-invalid hint=\"use an http or https URL without credentials, query, or fragment\""
        ),
        PairingError::InvitationTooLarge => {
            anyhow!("error pairing-qr-too-large hint=\"shorten sync.auth_token or the server URL\"")
        }
        PairingError::InvitationInvalid => anyhow!(
            "error pairing-invitation-invalid hint=\"check sync.server_url and sync.auth_token\""
        ),
    }
}

fn output_options<F>(
    stdout_is_terminal: bool,
    no_color_present: bool,
    terminal_size: F,
) -> (Option<usize>, bool)
where
    F: FnOnce() -> Option<u16>,
{
    if !stdout_is_terminal {
        return (None, false);
    }
    (
        terminal_size()
            .map(usize::from)
            .filter(|columns| *columns > 0),
        !no_color_present,
    )
}

fn render_terminal_qr(
    qr: &PairingQr,
    terminal_columns: Option<usize>,
    styled: bool,
) -> Result<String> {
    if let Some(columns) = terminal_columns
        && columns < qr.width()
    {
        bail!(
            "error pairing-terminal-too-narrow required_columns={} available_columns={} hint=\"widen the terminal and retry\"",
            qr.width(),
            columns
        );
    }

    let mut rendered = String::with_capacity(qr.rows().len() * (qr.width() + 16));
    for row in qr.rows() {
        if styled {
            rendered.push_str(QR_LINE_STYLE);
        }
        rendered.push_str(row);
        if styled {
            rendered.push_str(STYLE_RESET);
        }
        rendered.push('\n');
    }
    Ok(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SERVER: &str = "https://sync.example.test:8443/aven";
    const TEST_TOKEN: &str = "pairing-token-fixture-0123456789";

    fn presentation() -> crate::pairing::PairingPresentation {
        crate::pairing::PairingPresentation::new(TEST_SERVER.to_string(), TEST_TOKEN.to_string())
            .unwrap()
    }

    #[test]
    fn copy_confirmation_uses_bold_default_foreground_only_when_styled() {
        let origin = "http://10.0.0.1:3746";
        let plain = "Pairing invitation copied for http://10.0.0.1:3746.";
        assert_eq!(render_copy_confirmation(origin, false), plain);
        assert_eq!(
            render_copy_confirmation(origin, true),
            format!("\x1b[1;39m{plain}\x1b[0m")
        );
    }

    #[test]
    fn output_options_only_measure_and_style_real_terminals() {
        let redirected = output_options(false, false, || panic!("redirected output measured tty"));
        assert_eq!(redirected, (None, false));
        assert_eq!(output_options(true, false, || Some(120)), (Some(120), true));
        assert_eq!(output_options(true, true, || Some(120)), (Some(120), false));
        assert_eq!(output_options(true, false, || None), (None, true));
    }

    #[test]
    fn terminal_render_honors_width_and_style_modes() {
        let presentation = presentation();
        let qr = presentation.qr();
        let width = qr.width();

        let styled = render_terminal_qr(qr, Some(width), true).unwrap();
        assert!(
            styled
                .lines()
                .all(|line| { line.starts_with(QR_LINE_STYLE) && line.ends_with(STYLE_RESET) })
        );

        let plain = render_terminal_qr(qr, Some(width), false).unwrap();
        assert!(!plain.contains('\u{1b}'));
        assert_eq!(plain.lines().count(), qr.rows().len());
        assert!(render_terminal_qr(qr, None, false).is_ok());

        let error = render_terminal_qr(qr, Some(width - 1), false).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("pairing-terminal-too-narrow"));
        assert!(!message.contains(TEST_TOKEN));
        assert!(!message.contains("aven://pair/"));
    }
}
