//! Checking that a downloaded update comes from the same developer as the
//! running Aven, before anything runs it. The release checksum only shows the
//! download is intact, and Gatekeeper never sees what the updater installs.

use std::path::Path;

use anyhow::{Context, Result, bail};
use tokio::process::Command;

const CODESIGN: &str = "/usr/bin/codesign";

/// Fails unless `staged`, an executable or an app bundle, satisfies the
/// running executable's designated requirement. A running executable without
/// a Developer ID signature, such as a local build, has none to hold the
/// update to.
pub(super) async fn verify_signature(staged: &Path) -> Result<()> {
    let running = std::env::current_exe().context("locate the running aven")?;
    match developer_requirement(&running).await? {
        Some(requirement) => satisfies(staged, &requirement).await,
        None => Ok(()),
    }
}

async fn developer_requirement(executable: &Path) -> Result<Option<String>> {
    let output = Command::new(CODESIGN)
        .args(["--display", "-r-"])
        .arg(executable)
        .output()
        .await
        .context("run codesign")?;
    if !output.status.success() {
        return Ok(None);
    }
    Ok(designated_requirement(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// The requirement in `codesign --display -r-` output, if it names a
/// certificate from Apple's Developer ID chain. Ad hoc signatures print theirs
/// as a comment.
fn designated_requirement(display: &str) -> Option<String> {
    display
        .lines()
        .find_map(|line| line.strip_prefix("designated => "))
        .filter(|requirement| requirement.contains("anchor apple generic"))
        .map(str::to_string)
}

async fn satisfies(staged: &Path, requirement: &str) -> Result<()> {
    let output = Command::new(CODESIGN)
        .args(["--verify", "--strict", "--deep"])
        .arg(format!("-R={requirement}"))
        .arg(staged)
        .output()
        .await
        .context("run codesign")?;
    if !output.status.success() {
        let reason = String::from_utf8_lossy(&output.stderr);
        bail!(
            "the update is not signed by Aven's developer: {}",
            reason.trim()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_developer_id_requirement_counts() {
        let developer = "designated => identifier \"fi.zendit.Aven\" and anchor apple generic \
                         and certificate leaf[subject.OU] = YG824X87Y2\n";
        assert_eq!(
            designated_requirement(developer).as_deref(),
            Some(
                "identifier \"fi.zendit.Aven\" and anchor apple generic \
                 and certificate leaf[subject.OU] = YG824X87Y2"
            )
        );
        assert_eq!(
            designated_requirement("# designated => cdhash H\"843c49c5\"\n"),
            None
        );
        assert_eq!(
            designated_requirement("designated => identifier \"com.apple.ls\" and anchor apple\n"),
            None
        );
    }

    #[tokio::test]
    async fn code_must_meet_the_requirement() {
        satisfies(Path::new("/bin/ls"), "anchor apple")
            .await
            .unwrap();

        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("aven");
        std::fs::write(&script, "#!/bin/sh\necho aven 9.9.9\n").unwrap();
        let error = satisfies(&script, "anchor apple").await.unwrap_err();
        assert!(error.to_string().contains("not signed by Aven's developer"));
    }

    #[tokio::test]
    async fn an_unsigned_running_aven_checks_nothing() {
        // Test binaries carry only the linker's ad hoc signature.
        let temp = tempfile::tempdir().unwrap();
        verify_signature(&temp.path().join("missing"))
            .await
            .unwrap();
    }
}
