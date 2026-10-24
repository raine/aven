use bytes::Bytes;
use futures::StreamExt;
use rig_core::http_client::{self, HttpClientExt, LazyBody, MultipartForm, StreamingResponse};

const DEFAULT_MAX_PROVIDER_RESPONSE_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct BoundedHttpClient {
    client: reqwest::Client,
    max_response_bytes: usize,
}

impl Default for BoundedHttpClient {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            max_response_bytes: DEFAULT_MAX_PROVIDER_RESPONSE_BYTES,
        }
    }
}

impl BoundedHttpClient {
    async fn response<U>(
        response: reqwest::Response,
        max_response_bytes: usize,
    ) -> http_client::Result<http::Response<LazyBody<U>>>
    where
        U: From<Bytes> + Send + 'static,
    {
        let status = response.status();
        let headers = response.headers().clone();
        if !status.is_success() {
            return Err(http_client::Error::InvalidStatusCodeWithDetails {
                status,
                body: "provider request failed".to_string(),
                headers: Box::new(headers),
            });
        }
        let body: LazyBody<U> = Box::pin(async move {
            let bytes = read_bounded(response, max_response_bytes).await?;
            Ok(U::from(bytes))
        });
        let mut builder = http::Response::builder().status(status);
        *builder.headers_mut().ok_or(http_client::Error::NoHeaders)? = headers;
        builder.body(body).map_err(http_client::Error::Protocol)
    }
}

impl HttpClientExt for BoundedHttpClient {
    fn send<T, U>(
        &self,
        request: http::Request<T>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        T: Into<Bytes> + Send,
        U: From<Bytes> + Send + 'static,
    {
        let max_response_bytes = self.max_response_bytes;
        let (parts, body) = request.into_parts();
        let request = self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body.into());
        async move {
            let response = request.send().await.map_err(instance_error)?;
            Self::response(response, max_response_bytes).await
        }
    }

    #[allow(clippy::manual_async_fn)]
    fn send_multipart<U>(
        &self,
        _request: http::Request<MultipartForm>,
    ) -> impl Future<Output = http_client::Result<http::Response<LazyBody<U>>>> + Send + 'static
    where
        U: From<Bytes> + Send + 'static,
    {
        async { Err(unsupported_transport_error()) }
    }

    async fn send_streaming<T>(
        &self,
        request: http::Request<T>,
    ) -> http_client::Result<StreamingResponse>
    where
        T: Into<Bytes> + Send,
    {
        let (parts, body) = request.into_parts();
        let response = self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .body(body.into())
            .send()
            .await
            .map_err(instance_error)?;
        if !response.status().is_success() {
            return Err(http_client::Error::InvalidStatusCodeWithDetails {
                status: response.status(),
                body: "provider request failed".into(),
                headers: Box::new(response.headers().clone()),
            });
        }
        let mut builder = http::Response::builder().status(response.status());
        *builder.headers_mut().ok_or(http_client::Error::NoHeaders)? = response.headers().clone();
        let limit = self.max_response_bytes;
        let stream = response.bytes_stream().scan(0usize, move |total, chunk| {
            let chunk = chunk.map_err(instance_error).and_then(|chunk| {
                *total = total.saturating_add(chunk.len());
                if *total > limit {
                    Err(limit_error())
                } else {
                    Ok(chunk)
                }
            });
            std::future::ready(Some(chunk))
        });
        builder
            .body(Box::pin(stream) as rig_core::http_client::sse::BoxedStream)
            .map_err(http_client::Error::Protocol)
    }
}

async fn read_bounded(
    response: reqwest::Response,
    max_response_bytes: usize,
) -> http_client::Result<Bytes> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(instance_error)?;
        if body.len().saturating_add(chunk.len()) > max_response_bytes {
            return Err(limit_error());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn instance_error(error: reqwest::Error) -> http_client::Error {
    http_client::Error::Instance(error.into())
}

fn limit_error() -> http_client::Error {
    http_client::Error::Instance(
        std::io::Error::other("provider response exceeded service limit").into(),
    )
}

fn unsupported_transport_error() -> http_client::Error {
    http_client::Error::Instance(std::io::Error::other("provider transport is unsupported").into())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    #[tokio::test]
    async fn streaming_delivers_before_eof_and_bounds_wire_bytes() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (release, wait) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            socket.read(&mut request).unwrap();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\none\r\n").unwrap();
            socket.flush().unwrap();
            wait.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            let _ = write!(socket, "4\r\nmore\r\n0\r\n\r\n");
        });
        let client = BoundedHttpClient {
            max_response_bytes: 6,
            ..Default::default()
        };
        let request = http::Request::post(format!("http://{addr}/stream"))
            .body(Bytes::new())
            .unwrap();
        let mut body = client.send_streaming(request).await.unwrap().into_body();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), body.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(first, Bytes::from_static(b"one"));
        release.send(()).unwrap();
        assert!(body.next().await.unwrap().is_err());
        server.join().unwrap();
    }

    #[tokio::test]
    async fn bounds_success_bodies_and_discards_provider_error_text() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            for (status, body) in [
                (
                    "200 OK",
                    "x".repeat(DEFAULT_MAX_PROVIDER_RESPONSE_BYTES + 1),
                ),
                (
                    "429 Too Many Requests",
                    "provider-secret-marker".to_string(),
                ),
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0u8; 1024];
                let _ = stream.read(&mut request).unwrap();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        let client = BoundedHttpClient::default();
        let request = http::Request::post(format!("http://{addr}/large"))
            .body(Bytes::new())
            .unwrap();
        let response: http_client::Result<http::Response<LazyBody<Bytes>>> =
            client.send(request).await;
        assert!(response.unwrap().into_body().await.is_err());

        let request = http::Request::post(format!("http://{addr}/error"))
            .body(Bytes::new())
            .unwrap();
        let error = match client.send::<_, Bytes>(request).await {
            Ok(_) => panic!("provider error response should fail"),
            Err(error) => error.to_string(),
        };
        assert!(!error.contains("provider-secret-marker"));
        assert!(error.contains("provider request failed"));
        server.join().unwrap();
    }
}
