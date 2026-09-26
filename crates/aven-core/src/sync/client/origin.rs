//! Server origins that encrypted sync accepts.
use anyhow::{Context, Result, ensure};
use url::Url;

/// Longest server origin; invitations carry its length in one byte.
pub const MAX_SERVER_BYTES: usize = 255;

/// Validates `origin` for encrypted transport and returns the endpoint at
/// `path`: HTTP or HTTPS with a host and no credentials, query, fragment or
/// path.
pub(crate) fn endpoint(origin: &str, path: &str) -> Result<Url> {
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
    url.set_path(path);
    Ok(url)
}

/// Validates a server URL for encrypted transport and returns its origin.
pub fn server_origin(url: &str) -> Result<String> {
    endpoint(url, "/").context(
        "error sync-server-url-invalid hint=\"use an HTTP or HTTPS origin with no path, query or credentials\"",
    )?;
    let origin = Url::parse(url)?.origin().ascii_serialization();
    ensure!(
        origin.len() <= MAX_SERVER_BYTES,
        "error sync-server-url-too-long hint=\"use a server origin of at most 255 bytes\""
    );
    Ok(origin)
}

#[cfg(test)]
mod tests {
    use super::*;

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
