use crate::capability::{Capability, require};
use anyhow::Result;
use std::future::Future;

pub struct Request {
    pub path: &'static str,
    pub content_type: &'static str,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn json(
        path: &'static str,
        bearer: Option<&aven_protocol::claim::Secret>,
        value: &impl serde::Serialize,
    ) -> Self {
        Self {
            path,
            content_type: "application/json",
            authorization: bearer.map(|secret| format!("Bearer {}", hex::encode(secret.expose()))),
            body: serde_json::to_vec(value).unwrap(),
        }
    }
}

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn refusal(&self, status: u16, code: &str) {
        assert_eq!(self.status, status);
        assert_eq!(self.body, format!(r#"{{"error":"{code}"}}"#).as_bytes());
    }

    pub fn json<T: serde::de::DeserializeOwned>(&self) -> T {
        assert_eq!(self.status, 200, "{}", String::from_utf8_lossy(&self.body));
        serde_json::from_slice(&self.body).unwrap()
    }
}

#[derive(Clone, Copy)]
pub enum Delivery {
    Reply,
    LoseReply,
}

/// Test-only HTTP and fault controls. Each implementation owns disposable storage.
/// A lost reply discards a completed response, never rolls back its operation.
pub trait Driver: Sync {
    fn capabilities(&self) -> &[Capability];
    fn provision(
        &self,
        setup: [u8; 32],
        secret: &aven_protocol::claim::Secret,
        expiry: u64,
    ) -> impl Future<Output = Result<()>> + Send;
    fn request(
        &self,
        request: Request,
        delivery: Delivery,
    ) -> impl Future<Output = Result<Option<Response>>> + Send;
    fn restart(&mut self) -> impl Future<Output = Result<()>> + Send;
    fn set_time(&self, seconds: u64) -> Result<()>;
    fn arm_barrier(&self) -> Result<()>;
    fn wait_barrier(&self) -> impl Future<Output = Result<()>> + Send;
    fn release_barrier(&self) -> Result<()>;
}

pub fn required(driver: &impl Driver, capabilities: &[Capability]) {
    assert_eq!(
        require(driver.capabilities(), capabilities),
        Ok(()),
        "required scenario hooks are absent"
    );
}

pub async fn exchange(driver: &impl Driver, request: Request) -> Response {
    driver
        .request(request, Delivery::Reply)
        .await
        .unwrap()
        .expect("reply unexpectedly lost")
}
