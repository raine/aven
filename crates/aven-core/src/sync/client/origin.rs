//! Server origins that encrypted sync accepts.
use anyhow::{Context, Result, ensure};
use url::Url;

const MAX_SERVER_BYTES: usize = 2048;

/// Validates `origin` for encrypted transport and returns the endpoint at
/// `path`: HTTPS, or HTTP only with a loopback host, with no credentials,
/// query, fragment or path.
pub(crate) fn endpoint(origin: &str, path: &str) -> Result<Url> {
    let mut url = Url::parse(origin).map_err(|_| anyhow::anyhow!("error bootstrap-origin"))?;
    let loopback = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
            || h == "[::1]"
    });
    ensure!(
        (url.scheme() == "https" || (url.scheme() == "http" && loopback))
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
        "error sync-server-url-invalid hint=\"use an origin such as https://sync.example.com, or http:// only with a loopback address; no path, query or credentials\"",
    )?;
    let origin = Url::parse(url)?.origin().ascii_serialization();
    ensure!(
        origin.len() <= MAX_SERVER_BYTES,
        "error sync-server-url-too-long"
    );
    Ok(origin)
}
