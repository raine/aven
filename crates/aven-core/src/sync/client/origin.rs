//! Server origins that encrypted sync accepts.
use anyhow::{Context, Result, ensure};
use url::Url;

/// Longest server origin; invitations carry its length in one byte.
pub const MAX_SERVER_BYTES: usize = 255;

/// The official Aven Cloud sync host, which serves only HTTPS.
const AVEN_CLOUD_HOST: &str = "sync.aventasks.dev";

/// Validates `origin` for encrypted transport and returns the endpoint at
/// `path`: HTTP or HTTPS with a host and no credentials, query, fragment or
/// path. The official Cloud host requires HTTPS on every port.
pub(crate) fn endpoint(origin: &str, path: &str) -> Result<Url> {
    // Inspect the supplied authority/path too: URL parsing erases empty
    // userinfo and resolves dot segments before exposing those fields.
    let (_, address) = origin
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("error bootstrap-origin"))?;
    let (authority, suffix) = address.split_once('/').unwrap_or((address, ""));
    ensure!(
        !authority.contains('@') && !origin.contains('\\') && suffix.is_empty(),
        "error bootstrap-origin"
    );
    let mut url = Url::parse(origin).map_err(|_| anyhow::anyhow!("error bootstrap-origin"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "error bootstrap-origin"
    );
    // The parsed host is lowercase and percent-decoded; a trailing dot names
    // the same DNS host.
    let host = url.host_str().unwrap_or_default();
    ensure!(
        url.scheme() == "https" || host.strip_suffix('.').unwrap_or(host) != AVEN_CLOUD_HOST,
        "error sync-cloud-https-required"
    );
    url.set_path(path);
    Ok(url)
}

/// Validates a server URL for encrypted transport and returns its origin.
pub fn server_origin(url: &str) -> Result<String> {
    let url = endpoint(url, "/").context(
        "error sync-server-url-invalid hint=\"use an HTTP or HTTPS origin with no path, query or credentials\"",
    )?;
    let origin = url.origin().ascii_serialization();
    ensure!(
        origin.len() <= MAX_SERVER_BYTES,
        "error sync-server-url-too-long hint=\"use a server origin of at most 255 bytes\""
    );
    Ok(origin)
}

/// Identifies Cloud only after validating the complete server origin.
/// The address must still be displayed alongside this hosting label.
pub fn is_aven_cloud(origin: &str) -> bool {
    endpoint(origin, "/").is_ok_and(|url| {
        let host = url.host_str().unwrap_or_default();
        host.strip_suffix('.').unwrap_or(host) == AVEN_CLOUD_HOST
            && url.port_or_known_default() == Some(443)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_identity_uses_the_validated_origin() {
        for origin in [
            "https://sync.aventasks.dev",
            "HTTPS://SYNC.AVENTASKS.DEV:443/",
            "https://sync.aventasks.dev./",
            "https://%73ync.aventasks.dev",
        ] {
            assert!(is_aven_cloud(origin), "{origin}");
        }
        for origin in [
            "http://sync.aventasks.dev",
            "https://sync.aventasks.dev:444",
            "https://sync.aventasks.dev.evil.example",
            "https://sync.aventasks.dev/private",
            "https://user:secret@sync.aventasks.dev",
            "https://sync.aventasks.dev/?x=1",
            "https://sync.aventasks.dev/#x",
            "https://@sync.aventasks.dev",
            "https://sync.aventasks.dev/private/..",
            "https://sync.aventasks.dev/.",
        ] {
            assert!(!is_aven_cloud(origin), "{origin}");
        }
        for origin in [
            "http://SYNC.AVENTASKS.DEV:443",
            "http://sync.aventasks.dev.:80",
            "http://%73ync.aventasks.dev:3746",
        ] {
            assert!(endpoint(origin, "/e2ee/bootstrap/v1").is_err(), "{origin}");
        }
    }

    #[test]
    fn accepts_http_and_https_origins() {
        for origin in [
            "http://127.0.0.1:3746",
            "http://100.100.20.30:3746",
            "http://sync.private.example:3746",
            "https://sync.example.com",
        ] {
            assert_eq!(server_origin(origin).unwrap(), origin);
        }
    }

    #[test]
    fn official_cloud_host_requires_https_without_changing_origins() {
        for origin in [
            "http://sync.aventasks.dev",
            "HTTP://SYNC.AVENTASKS.DEV",
            "http://sync.aventasks.dev:80",
            "http://sync.aventasks.dev:443",
            "http://sync.aventasks.dev:3746",
            "http://sync.aventasks.dev.",
            "http://%73ync.aventasks.dev",
        ] {
            let error = endpoint(origin, "/e2ee/bootstrap/v1").unwrap_err();
            assert_eq!(
                error.to_string(),
                "error sync-cloud-https-required",
                "{origin}"
            );
            let error = server_origin(origin).unwrap_err();
            assert!(
                format!("{error:#}").contains("sync-cloud-https-required"),
                "{origin}: {error:#}"
            );
        }
        for (origin, canonical) in [
            ("https://sync.aventasks.dev", "https://sync.aventasks.dev"),
            (
                "HTTPS://SYNC.AVENTASKS.DEV:443/",
                "https://sync.aventasks.dev",
            ),
            ("https://sync.aventasks.dev.", "https://sync.aventasks.dev."),
            (
                "https://sync.aventasks.dev:444",
                "https://sync.aventasks.dev:444",
            ),
            (
                "http://sync.aventasks.dev.example",
                "http://sync.aventasks.dev.example",
            ),
            (
                "http://dev-sync.aventasks.dev",
                "http://dev-sync.aventasks.dev",
            ),
        ] {
            assert_eq!(server_origin(origin).unwrap(), canonical, "{origin}");
        }
    }

    #[test]
    fn rejects_non_http_and_non_origin_urls() {
        for origin in [
            "ftp://sync.example.com",
            "https://user:secret@sync.example.com",
            "https://sync.example.com/private",
            "https://sync.example.com/?token=secret",
            "https://sync.example.com/#fragment",
            "https://",
            "not a URL",
        ] {
            let error = server_origin(origin).unwrap_err();
            assert!(
                format!("{error:#}").contains("sync-server-url-invalid"),
                "{origin}: {error:#}"
            );
        }
    }
}
