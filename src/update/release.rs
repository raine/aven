use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use semver::Version;
use serde::Deserialize;

use super::{REPOSITORY, Release};

const RELEASE_API: &str = "https://api.github.com/repos/raine/aven/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FetchResult {
    NotModified {
        etag: Option<String>,
    },
    Release {
        release: Release,
        etag: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

pub(super) async fn fetch_latest(
    client: &reqwest::Client,
    etag: Option<&str>,
) -> Result<FetchResult> {
    let mut request = client.get(RELEASE_API);
    if let Some(etag) = etag {
        request = request.header(IF_NONE_MATCH, etag);
    }
    let response = request.send().await.context("check GitHub releases")?;
    let response_etag = response
        .headers()
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(FetchResult::NotModified {
            etag: response_etag.or_else(|| etag.map(str::to_string)),
        });
    }
    if response.status() == StatusCode::FORBIDDEN
        || response.status() == StatusCode::TOO_MANY_REQUESTS
    {
        bail!("GitHub rate limit reached; try again later");
    }
    let response = response
        .error_for_status()
        .context("fetch latest GitHub release")?;
    let body = response
        .bytes()
        .await
        .context("read latest GitHub release")?;
    if body.len() > 1024 * 1024 {
        bail!("GitHub release response is unexpectedly large");
    }
    let release: GithubRelease =
        serde_json::from_slice(&body).context("parse latest GitHub release")?;
    Ok(FetchResult::Release {
        release: parse_release(release)?,
        etag: response_etag,
    })
}

fn parse_release(release: GithubRelease) -> Result<Release> {
    let raw_version = release
        .tag_name
        .strip_prefix('v')
        .unwrap_or(&release.tag_name);
    let version = Version::parse(raw_version)
        .with_context(|| format!("release tag {} is not valid semver", release.tag_name))?;
    let archive_name = platform_archive_name()?;
    let checksum_name = archive_name.replace(".tar.gz", ".sha256");
    let archive_url = unique_asset_url(&release.assets, &archive_name)?;
    let checksum_url = unique_asset_url(&release.assets, &checksum_name)?;
    validate_asset_url(&archive_url)?;
    validate_asset_url(&checksum_url)?;

    Ok(Release {
        version,
        tag: release.tag_name,
        archive_name,
        archive_url,
        checksum_url,
    })
}

fn unique_asset_url(assets: &[GithubAsset], name: &str) -> Result<String> {
    let matches = assets
        .iter()
        .filter(|asset| asset.name == name)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [asset] => Ok(asset.browser_download_url.clone()),
        [] => bail!("release is missing asset {name}"),
        _ => bail!("release contains duplicate asset {name}"),
    }
}

fn validate_asset_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("parse release asset URL")?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("github.com") {
        bail!("release asset URL is not an HTTPS GitHub URL");
    }
    let expected_prefix = format!("/{REPOSITORY}/releases/download/");
    if !parsed.path().starts_with(&expected_prefix) {
        bail!("release asset URL does not belong to {REPOSITORY}");
    }
    Ok(())
}

pub(super) fn platform_archive_name() -> Result<String> {
    let suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => bail!("aven releases do not support {os}/{arch}"),
    };
    Ok(format!("aven-{suffix}.tar.gz"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!(
                "https://github.com/raine/aven/releases/download/v1.2.3/{name}"
            ),
        }
    }

    #[test]
    fn parses_release_and_strips_v_for_semver() {
        let archive = platform_archive_name().unwrap();
        let checksum = archive.replace(".tar.gz", ".sha256");
        let parsed = parse_release(GithubRelease {
            tag_name: "v1.2.3".to_string(),
            assets: vec![asset(&archive), asset(&checksum)],
        })
        .unwrap();

        assert_eq!(parsed.version, Version::new(1, 2, 3));
        assert_eq!(parsed.tag, "v1.2.3");
        assert_eq!(parsed.archive_name, archive);
    }

    #[test]
    fn rejects_malformed_version_and_missing_or_duplicate_assets() {
        let archive = platform_archive_name().unwrap();
        let checksum = archive.replace(".tar.gz", ".sha256");
        assert!(
            parse_release(GithubRelease {
                tag_name: "latest".to_string(),
                assets: vec![asset(&archive), asset(&checksum)],
            })
            .is_err()
        );
        assert!(
            parse_release(GithubRelease {
                tag_name: "v1.2.3".to_string(),
                assets: vec![asset(&archive)],
            })
            .is_err()
        );
        assert!(
            parse_release(GithubRelease {
                tag_name: "v1.2.3".to_string(),
                assets: vec![asset(&archive), asset(&archive), asset(&checksum)],
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_non_github_asset_urls() {
        assert!(validate_asset_url("http://github.com/raine/aven/file").is_err());
        assert!(validate_asset_url("https://example.com/raine/aven/file").is_err());
        assert!(
            validate_asset_url("https://github.com/other/repo/releases/download/v1/a").is_err()
        );
    }
}
