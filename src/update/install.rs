use std::io::{Cursor, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, atomic::AtomicBool};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::eligibility::can_stage_beside;
use super::{InstallPlan, InstallSuccess, UpdatePhase, UpdateProgress, ensure_not_cancelled};

const ARCHIVE_LIMIT: usize = 64 * 1024 * 1024;
const CHECKSUM_LIMIT: usize = 4096;
const BINARY_LIMIT: usize = 128 * 1024 * 1024;

pub(crate) async fn install_direct(
    client: reqwest::Client,
    plan: InstallPlan,
    progress: watch::Sender<UpdateProgress>,
    cancelled: Arc<AtomicBool>,
) -> Result<InstallSuccess> {
    let target = plan
        .direct_target()
        .context("this installation cannot be updated directly")?
        .to_path_buf();
    ensure_not_cancelled(&cancelled)?;
    set_phase(&progress, UpdatePhase::Downloading);
    let archive = download_limited(&client, &plan.release.archive_url, ARCHIVE_LIMIT).await?;
    ensure_not_cancelled(&cancelled)?;
    let checksum = download_limited(&client, &plan.release.checksum_url, CHECKSUM_LIMIT).await?;

    ensure_not_cancelled(&cancelled)?;
    set_phase(&progress, UpdatePhase::Verifying);
    verify_checksum(&archive, &checksum, &plan.release.archive_name)?;

    ensure_not_cancelled(&cancelled)?;
    set_phase(&progress, UpdatePhase::Extracting);
    let binary = extract_binary(&archive)?;

    ensure_not_cancelled(&cancelled)?;
    set_phase(&progress, UpdatePhase::Staging);
    let staged = stage_binary(&target, &binary)?.into_temp_path();
    validate_staged_binary(&staged, &plan.release.version).await?;

    ensure_not_cancelled(&cancelled)?;
    if !can_stage_beside(&target) {
        bail!("the install directory is not writable");
    }
    set_phase(&progress, UpdatePhase::Installing);
    let staged_path = staged.keep().context("retain staged update")?;
    if let Err(error) = std::fs::rename(&staged_path, &target) {
        let _ = std::fs::remove_file(&staged_path);
        return Err(error).context("replace aven executable");
    }

    Ok(InstallSuccess {
        version: plan.release.version,
    })
}

fn set_phase(progress: &watch::Sender<UpdateProgress>, phase: UpdatePhase) {
    progress.send_replace(UpdateProgress { phase });
}

async fn download_limited(client: &reqwest::Client, url: &str, limit: usize) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("download {url}"))?
        .error_for_status()
        .with_context(|| format!("download {url}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        bail!("download is larger than the allowed limit");
    }
    let bytes = response.bytes().await.context("read release download")?;
    if bytes.len() > limit {
        bail!("download is larger than the allowed limit");
    }
    Ok(bytes.to_vec())
}

fn verify_checksum(archive: &[u8], checksum: &[u8], archive_name: &str) -> Result<()> {
    let checksum = std::str::from_utf8(checksum).context("checksum file is not UTF-8")?;
    let mut fields = checksum.split_whitespace();
    let expected = fields.next().context("checksum file is empty")?;
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("checksum file does not contain a valid SHA-256 value");
    }
    if let Some(filename) = fields.next()
        && filename.trim_start_matches('*') != archive_name
    {
        bail!("checksum file names an unexpected archive");
    }
    if fields.next().is_some() {
        bail!("checksum file has unexpected fields");
    }

    let actual = hex::encode(Sha256::digest(archive));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("release checksum does not match the downloaded archive");
    }
    Ok(())
}

fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    let decoder = GzDecoder::new(Cursor::new(archive));
    let mut archive = tar::Archive::new(decoder);
    let mut binary = None;
    for entry in archive.entries().context("read release archive")? {
        let entry = entry.context("read release archive entry")?;
        let path = entry.path().context("read release archive path")?;
        if path.as_ref() != Path::new("aven") || !entry.header().entry_type().is_file() {
            bail!("release archive must contain only a regular file named aven");
        }
        if binary.is_some() {
            bail!("release archive contains more than one aven binary");
        }
        let size = entry.size();
        if size > BINARY_LIMIT as u64 {
            bail!("release binary is unexpectedly large");
        }
        let mut bytes = Vec::with_capacity(size as usize);
        entry
            .take(BINARY_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .context("extract aven binary")?;
        if bytes.len() > BINARY_LIMIT {
            bail!("release binary is unexpectedly large");
        }
        binary = Some(bytes);
    }
    binary.context("release archive does not contain aven")
}

fn stage_binary(target: &Path, binary: &[u8]) -> Result<tempfile::NamedTempFile> {
    let parent = target
        .parent()
        .context("aven executable has no parent directory")?;
    let mut staged = tempfile::Builder::new()
        .prefix(".aven-update-")
        .tempfile_in(parent)
        .context("stage update beside aven executable")?;
    staged.write_all(binary).context("write staged update")?;
    staged
        .as_file()
        .set_permissions(std::fs::Permissions::from_mode(0o755))
        .context("make staged update executable")?;
    staged.as_file().sync_all().context("sync staged update")?;
    Ok(staged)
}

async fn validate_staged_binary(path: &Path, expected: &Version) -> Result<()> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(path).arg("--version").output(),
    )
    .await
    .context("staged aven validation timed out")?
    .context("run staged aven")?;
    if !output.status.success() {
        bail!("staged aven failed its version check");
    }
    let stdout = String::from_utf8(output.stdout).context("staged aven version is not UTF-8")?;
    let actual = stdout
        .split_whitespace()
        .last()
        .context("staged aven did not print a version")?;
    if Version::parse(actual).ok().as_ref() != Some(expected) {
        bail!("staged aven reports v{actual}, expected v{expected}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};

    fn archive_with(path: &str, body: &[u8]) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut builder = tar::Builder::new(encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, path, Cursor::new(body))
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn checksum_requires_hash_and_expected_filename() {
        let archive = b"archive";
        let hash = hex::encode(Sha256::digest(archive));
        verify_checksum(
            archive,
            format!("{hash}  aven-test.tar.gz\n").as_bytes(),
            "aven-test.tar.gz",
        )
        .unwrap();
        assert!(verify_checksum(archive, b"bad", "aven-test.tar.gz").is_err());
        assert!(
            verify_checksum(
                archive,
                format!("{hash}  other.tar.gz\n").as_bytes(),
                "aven-test.tar.gz",
            )
            .is_err()
        );
    }

    #[test]
    fn extraction_accepts_only_one_root_binary() {
        assert_eq!(
            extract_binary(&archive_with("aven", b"binary")).unwrap(),
            b"binary"
        );
        assert!(extract_binary(&archive_with("bin/aven", b"binary")).is_err());
        assert!(extract_binary(&archive_with("other", b"binary")).is_err());
    }
}
