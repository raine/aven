use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use reqwest::header::{ETAG, IF_NONE_MATCH};
use semver::Version;
use serde::Deserialize;

use super::{REPOSITORY, Release, ReleaseArchive};

/// Debug builds read releases from this URL instead, so a local server can
/// stand in for GitHub. Its assets may then live on a loopback HTTP address.
#[cfg(debug_assertions)]
const RELEASE_API_OVERRIDE: &str = "AVEN_DEV_RELEASE_API";

fn release_api() -> String {
    #[cfg(debug_assertions)]
    if let Some(api) = release_api_override() {
        return api;
    }
    format!("https://api.github.com/repos/{REPOSITORY}/releases/latest")
}

#[cfg(debug_assertions)]
fn release_api_override() -> Option<String> {
    std::env::var(RELEASE_API_OVERRIDE)
        .ok()
        .filter(|api| !api.is_empty())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FetchResult {
    Unavailable,
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
    channel: ReleaseChannel,
) -> Result<FetchResult> {
    fetch_latest_at(client, etag, channel, &release_api()).await
}

async fn fetch_latest_at(
    client: &reqwest::Client,
    etag: Option<&str>,
    channel: ReleaseChannel,
    url: &str,
) -> Result<FetchResult> {
    let mut request = client.get(url);
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
    if response.status() == StatusCode::NOT_FOUND && channel == ReleaseChannel::Desktop {
        return Ok(FetchResult::Unavailable);
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
    if channel == ReleaseChannel::Desktop
        && let Some(name) = channel_app_archive_name(channel)?
        && !release.assets.iter().any(|asset| asset.name == name)
    {
        return Ok(FetchResult::Unavailable);
    }
    Ok(FetchResult::Release {
        release: parse_release(release, channel)?,
        etag: response_etag,
    })
}

fn parse_release(release: GithubRelease, channel: ReleaseChannel) -> Result<Release> {
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
    let app = match (channel, channel_app_archive_name(channel)?) {
        (ReleaseChannel::Terminal, Some(name)) => optional_archive(&release.assets, name)?,
        (ReleaseChannel::Terminal, None) => None,
        // A Desktop install updates only from its own archive.
        (ReleaseChannel::Desktop, Some(name)) => {
            let missing = format!(
                "release {} has no Aven Desktop archive {name}",
                release.tag_name
            );
            Some(optional_archive(&release.assets, name)?.context(missing)?)
        }
        (ReleaseChannel::Desktop, None) => bail!("Aven Desktop has no release archive here"),
    };

    Ok(Release {
        version,
        tag: release.tag_name,
        archive_name,
        archive_url,
        checksum_url,
        app: app.map(Box::new),
    })
}

fn optional_archive(assets: &[GithubAsset], name: String) -> Result<Option<ReleaseArchive>> {
    if !assets.iter().any(|asset| asset.name == name) {
        return Ok(None);
    }
    let url = unique_asset_url(assets, &name)?;
    let checksum_url = unique_asset_url(assets, &name.replace(".tar.gz", ".sha256"))?;
    validate_asset_url(&url)?;
    validate_asset_url(&checksum_url)?;
    Ok(Some(ReleaseArchive {
        name,
        url,
        checksum_url,
    }))
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
    #[cfg(debug_assertions)]
    if release_api_override().is_some()
        && parsed.scheme() == "http"
        && parsed.host_str() == Some("127.0.0.1")
    {
        return Ok(());
    }
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
    Ok(format!("aven-{}.tar.gz", platform_suffix()?))
}

/// The product line an installation updates within. Each channel has its own
/// release archives and never falls back to the other channel's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseChannel {
    /// CLI, TUI and daemon without the desktop app.
    Terminal,
    /// The desktop app, which also provides the CLI, TUI and daemon.
    #[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
    Desktop,
}

/// Fails, before any request, where `channel` publishes nothing for this
/// platform.
pub(super) fn ensure_published(channel: ReleaseChannel) -> Result<()> {
    channel_app_archive_name(channel).map(drop)
}

/// The archive holding only `Aven.app` for `channel` on this platform, or
/// `None` where the terminal channel ships only the bare binary archive.
fn channel_app_archive_name(channel: ReleaseChannel) -> Result<Option<String>> {
    let suffix = platform_suffix()?;
    let macos = suffix.starts_with("darwin-");
    match channel {
        ReleaseChannel::Terminal => Ok(macos.then(|| format!("aven-{suffix}-app.tar.gz"))),
        ReleaseChannel::Desktop if macos => Ok(Some(format!("aven-desktop-{suffix}-app.tar.gz"))),
        ReleaseChannel::Desktop => bail!(
            "Aven Desktop has no Linux releases, so this desktop build can't update itself. \
             Rebuild it from a newer source checkout."
        ),
    }
}

fn platform_suffix() -> Result<&'static str> {
    Ok(match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-amd64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-amd64",
        (os, arch) => bail!("aven releases do not support {os}/{arch}"),
    })
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

    #[cfg(target_os = "macos")]
    async fn fetch_fixture(
        status: u16,
        body: &str,
        channel: ReleaseChannel,
    ) -> Result<FetchResult> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let response = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut byte = [0];
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            assert!(
                String::from_utf8(request)
                    .unwrap()
                    .to_lowercase()
                    .contains("if-none-match: previous")
            );
            if status != 0 {
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let result = fetch_latest_at(
            &reqwest::Client::builder().no_proxy().build()?,
            Some("previous"),
            channel,
            &url,
        )
        .await;
        server.await?;
        result
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn only_confirmed_absence_withdraws_a_desktop_release() {
        assert_eq!(
            fetch_fixture(404, "", ReleaseChannel::Desktop)
                .await
                .unwrap(),
            FetchResult::Unavailable
        );
        assert_eq!(
            fetch_fixture(
                200,
                r#"{"tag_name":"v9.0.0","assets":[]}"#,
                ReleaseChannel::Desktop
            )
            .await
            .unwrap(),
            FetchResult::Unavailable
        );
        assert_eq!(
            fetch_fixture(304, "", ReleaseChannel::Desktop)
                .await
                .unwrap(),
            FetchResult::NotModified {
                etag: Some("previous".into())
            }
        );
        for status in [0, 403, 429, 500, 502, 503] {
            assert!(
                fetch_fixture(status, "", ReleaseChannel::Desktop)
                    .await
                    .is_err(),
                "{status}"
            );
        }
        assert!(
            fetch_fixture(200, "invalid json", ReleaseChannel::Desktop)
                .await
                .is_err()
        );
        assert!(
            fetch_fixture(404, "", ReleaseChannel::Terminal)
                .await
                .is_err()
        );
        assert!(
            fetch_fixture(
                200,
                r#"{"tag_name":"v9.0.0","assets":[]}"#,
                ReleaseChannel::Terminal
            )
            .await
            .is_err()
        );
    }

    #[test]
    fn parses_release_and_strips_v_for_semver() {
        let archive = platform_archive_name().unwrap();
        let checksum = archive.replace(".tar.gz", ".sha256");
        let parsed = parse_release(
            GithubRelease {
                tag_name: "v1.2.3".to_string(),
                assets: vec![asset(&archive), asset(&checksum)],
            },
            ReleaseChannel::Terminal,
        )
        .unwrap();

        assert_eq!(parsed.version, Version::new(1, 2, 3));
        assert_eq!(parsed.tag, "v1.2.3");
        assert_eq!(parsed.archive_name, archive);
        assert_eq!(parsed.app, None);
    }

    #[test]
    fn parses_app_archive_only_on_macos() {
        let archive = platform_archive_name().unwrap();
        let checksum = archive.replace(".tar.gz", ".sha256");
        let app = archive.replace(".tar.gz", "-app.tar.gz");
        let app_checksum = archive.replace(".tar.gz", "-app.sha256");
        let parsed = parse_release(
            GithubRelease {
                tag_name: "v1.2.3".to_string(),
                assets: vec![
                    asset(&archive),
                    asset(&checksum),
                    asset(&app),
                    asset(&app_checksum),
                ],
            },
            ReleaseChannel::Terminal,
        )
        .unwrap();

        if cfg!(target_os = "macos") {
            let parsed_app = parsed.app.unwrap();
            assert_eq!(parsed_app.name, app);
            assert!(parsed_app.url.ends_with(&format!("/v1.2.3/{app}")));
            assert!(
                parsed_app
                    .checksum_url
                    .ends_with(&format!("/v1.2.3/{app_checksum}"))
            );
            assert!(
                parse_release(
                    GithubRelease {
                        tag_name: "v1.2.3".to_string(),
                        assets: vec![asset(&archive), asset(&checksum), asset(&app)],
                    },
                    ReleaseChannel::Terminal
                )
                .is_err(),
                "an app archive without its checksum must be rejected"
            );
            assert!(
                parse_release(
                    GithubRelease {
                        tag_name: "v1.2.3".to_string(),
                        assets: vec![
                            asset(&archive),
                            asset(&checksum),
                            asset(&app),
                            asset(&app),
                            asset(&app_checksum),
                        ],
                    },
                    ReleaseChannel::Terminal
                )
                .is_err()
            );
            let mut foreign = asset(&app);
            foreign.browser_download_url = format!("https://example.com/{app}");
            assert!(
                parse_release(
                    GithubRelease {
                        tag_name: "v1.2.3".to_string(),
                        assets: vec![
                            asset(&archive),
                            asset(&checksum),
                            foreign,
                            asset(&app_checksum),
                        ],
                    },
                    ReleaseChannel::Terminal
                )
                .is_err()
            );
        } else {
            assert_eq!(parsed.app, None);
        }
    }

    #[test]
    fn channels_select_separate_app_archives() {
        let suffix = platform_suffix().unwrap();
        let terminal = channel_app_archive_name(ReleaseChannel::Terminal).unwrap();
        let desktop = channel_app_archive_name(ReleaseChannel::Desktop);
        if cfg!(target_os = "macos") {
            assert_eq!(terminal, Some(format!("aven-{suffix}-app.tar.gz")));
            assert_eq!(
                desktop.unwrap(),
                Some(format!("aven-desktop-{suffix}-app.tar.gz"))
            );
        } else {
            assert_eq!(terminal, None);
            let error =
                desktop.expect_err("the desktop channel must not fall back to terminal archives");
            assert!(error.to_string().contains("no Linux releases"), "{error}");
            assert!(ensure_published(ReleaseChannel::Desktop).is_err());
            assert!(ensure_published(ReleaseChannel::Terminal).is_ok());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn each_channel_parses_only_its_own_app_archive() {
        let suffix = platform_suffix().unwrap();
        let named = |name: String| [asset(&name), asset(&name.replace(".tar.gz", ".sha256"))];
        let terminal = named(format!("aven-{suffix}-app.tar.gz"));
        let desktop = named(format!("aven-desktop-{suffix}-app.tar.gz"));
        let bare = named(platform_archive_name().unwrap());
        let release = |assets: &[&[GithubAsset; 2]]| GithubRelease {
            tag_name: "v1.2.3".to_string(),
            assets: assets
                .iter()
                .flat_map(|pair| pair.iter())
                .map(|asset| GithubAsset {
                    name: asset.name.clone(),
                    browser_download_url: asset.browser_download_url.clone(),
                })
                .collect(),
        };

        let both = [&bare, &terminal, &desktop];
        let desktop_app = parse_release(release(&both), ReleaseChannel::Desktop)
            .unwrap()
            .app
            .unwrap();
        assert_eq!(desktop_app.name, desktop[0].name);
        assert!(desktop_app.checksum_url.ends_with(&desktop[1].name));
        let terminal_app = parse_release(release(&both), ReleaseChannel::Terminal)
            .unwrap()
            .app
            .unwrap();
        assert_eq!(terminal_app.name, terminal[0].name);

        let error = parse_release(release(&[&bare, &terminal]), ReleaseChannel::Desktop)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no Aven Desktop archive"), "{error}");
        assert_eq!(
            parse_release(release(&[&bare, &desktop]), ReleaseChannel::Terminal)
                .unwrap()
                .app,
            None,
            "the terminal channel must never pick up the Desktop archive"
        );
    }

    #[test]
    fn rejects_malformed_version_and_missing_or_duplicate_assets() {
        let archive = platform_archive_name().unwrap();
        let checksum = archive.replace(".tar.gz", ".sha256");
        assert!(
            parse_release(
                GithubRelease {
                    tag_name: "latest".to_string(),
                    assets: vec![asset(&archive), asset(&checksum)],
                },
                ReleaseChannel::Terminal
            )
            .is_err()
        );
        assert!(
            parse_release(
                GithubRelease {
                    tag_name: "v1.2.3".to_string(),
                    assets: vec![asset(&archive)],
                },
                ReleaseChannel::Terminal
            )
            .is_err()
        );
        assert!(
            parse_release(
                GithubRelease {
                    tag_name: "v1.2.3".to_string(),
                    assets: vec![asset(&archive), asset(&archive), asset(&checksum)],
                },
                ReleaseChannel::Terminal
            )
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
