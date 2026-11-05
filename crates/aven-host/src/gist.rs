//! Secret task report publication through GitHub CLI.
use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command as ProcessCommand, Stdio};

use anyhow::{Context, Result, anyhow};

/// `gh gist create`, fed the report on standard input.
pub fn create_secret_gist(
    program: &OsStr,
    markdown: &str,
    filename: &str,
    description: &str,
) -> Result<String> {
    let mut child = ProcessCommand::new(program)
        .args([
            "gist",
            "create",
            "--filename",
            filename,
            "--desc",
            description,
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run gh; install and authenticate GitHub CLI")?;
    child
        .stdin
        .take()
        .context("gh standard input is unavailable")?
        .write_all(markdown.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if message.is_empty() {
            return Err(anyhow!("gh gist create exited with {}", output.status));
        }
        return Err(anyhow!(message));
    }
    let url = String::from_utf8(output.stdout)
        .context("gh gist create returned a non-UTF-8 URL")?
        .trim()
        .to_string();
    if url.is_empty() {
        return Err(anyhow!("gh gist create returned no URL"));
    }
    Ok(url)
}
