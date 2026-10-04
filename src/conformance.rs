//! SQLite implementation of the development-only HTTP scenario driver.
use anyhow::{Result, ensure};
use aven_conformance::{
    capability::Capability,
    driver::{Delivery, Driver, Request, Response},
};
use aven_core::{db::Database, sync::seed_claim::Secret};
use axum::{extract::Request as HttpRequest, middleware::Next, response::Response as HttpResponse};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use tokio::sync::Notify;

#[derive(Default)]
struct Gate {
    armed: AtomicBool,
    entered: Notify,
    released: Notify,
}

async fn barrier(
    axum::extract::State(gate): axum::extract::State<Arc<Gate>>,
    request: HttpRequest,
    next: Next,
) -> HttpResponse {
    if gate.armed.swap(false, Ordering::SeqCst) {
        gate.entered.notify_one();
        gate.released.notified().await;
    }
    next.run(request).await
}

struct Sqlite {
    root: tempfile::TempDir,
    db: Option<Database>,
    origin: String,
    server: Option<tokio::task::JoinHandle<()>>,
    clock: Arc<AtomicU64>,
    gate: Arc<Gate>,
    http: reqwest::Client,
}

impl Drop for Sqlite {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

impl Sqlite {
    async fn new(now: u64) -> Self {
        let mut driver = Self {
            root: tempfile::tempdir().unwrap(),
            db: None,
            origin: String::new(),
            server: None,
            clock: Arc::new(AtomicU64::new(now)),
            gate: Arc::new(Gate::default()),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        };
        driver.start().await.unwrap();
        driver
    }

    async fn start(&mut self) -> Result<()> {
        let db = Database::open(&self.root.path().join("server.sqlite")).await?;
        let router = crate::seed_bootstrap_http::router(db.clone(), Default::default())
            .merge(crate::peer_enrollment_http::router_with_clock(
                db.clone(),
                self.clock.clone(),
            ))
            .merge(crate::test_support::e2ee_http::tail_router(db.clone()))
            .layer(axum::middleware::from_fn_with_state(
                self.gate.clone(),
                barrier,
            ));
        let (origin, server) = crate::test_support::e2ee_http::serve(router, "127.0.0.1:0").await;
        self.db = Some(db);
        self.origin = origin;
        self.server = Some(server);
        Ok(())
    }
}

impl Driver for Sqlite {
    fn capabilities(&self) -> &[Capability] {
        &[
            Capability::ControlledClock,
            Capability::Barrier,
            Capability::LostReply,
            Capability::Restart,
        ]
    }

    async fn provision(&self, setup: [u8; 32], secret: &Secret, expiry: u64) -> Result<()> {
        self.db
            .as_ref()
            .unwrap()
            .issue_e2ee_server_setup(secret, setup, expiry)
            .await
            .map(|_| ())
    }

    async fn request(&self, request: Request, delivery: Delivery) -> Result<Option<Response>> {
        let mut http = self
            .http
            .post(format!("{}{}", self.origin, request.path))
            .header("content-type", request.content_type)
            .body(request.body);
        if let Some(authorization) = request.authorization {
            http = http.header("authorization", authorization);
        }
        let response = http.send().await?;
        ensure!(
            response
                .headers()
                .get("cache-control")
                .is_some_and(|value| value == "no-store"),
            "response is cacheable"
        );
        let status = response.status().as_u16();
        let body = response.bytes().await?.to_vec();
        Ok(match delivery {
            Delivery::Reply => Some(Response { status, body }),
            Delivery::LoseReply => None,
        })
    }

    async fn restart(&mut self) -> Result<()> {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        self.db.take();
        self.start().await
    }

    fn set_time(&self, seconds: u64) -> Result<()> {
        self.clock.store(seconds, Ordering::SeqCst);
        Ok(())
    }

    fn arm_barrier(&self) -> Result<()> {
        ensure!(
            !self.gate.armed.swap(true, Ordering::SeqCst),
            "barrier already armed"
        );
        Ok(())
    }

    async fn wait_barrier(&self) -> Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            self.gate.entered.notified(),
        )
        .await?;
        Ok(())
    }

    fn release_barrier(&self) -> Result<()> {
        self.gate.released.notify_one();
        Ok(())
    }
}

#[tokio::test]
async fn endpoint_refusal_ordering() {
    aven_conformance::refusal::ordering(&Sqlite::new(crate::test_support::e2ee_http::now()).await)
        .await;
}

#[tokio::test]
async fn claim_bootstrap_enrollment_tail() {
    let root = tempfile::tempdir().unwrap();
    let (client, store, seed, package) = crate::test_support::e2ee_http::fixture(root.path()).await;
    let keyring = store.load_required().unwrap();
    let publication = seed
        .prepare_bootstrap_publication(&package, keyring.package_key())
        .unwrap();
    let membership = aven_core::sync::seed_claim::membership::Membership::from_publication(
        seed.genesis(),
        &package.descriptor,
        publication.record(),
    )
    .unwrap();
    let keys = membership
        .verify_initial_key(keyring.package_key())
        .unwrap();
    let now = crate::test_support::e2ee_http::now();
    let mut driver = Sqlite::new(now).await;
    let setup = crate::test_support::e2ee_http::setup_secret();
    aven_conformance::claim::reissue_before_claim(&driver, seed.genesis(), &setup).await;
    aven_conformance::claim::retry_after_lost_reply(
        &mut driver,
        seed.genesis(),
        &setup,
        seed.bearer(),
    )
    .await;
    aven_conformance::bootstrap::completeness_and_retry(&mut driver, &seed, &package, &publication)
        .await;
    crate::seed_bootstrap_http::Client::new(&driver.origin)
        .unwrap()
        .resume(&store, &client)
        .await
        .unwrap();
    let (peer, current) = aven_conformance::enrollment::expiry_and_admission(
        &mut driver,
        &seed,
        &membership,
        &keys,
        now,
    )
    .await;
    aven_conformance::tail::admitted_peer(&driver, &peer, &current).await;
    crate::peer_enrollment_http::Client::new(&driver.origin)
        .unwrap()
        .refresh(&store, &client)
        .await
        .unwrap();
    let inputs = store.tail_inputs(&client, &driver.origin).await.unwrap();
    let frozen = client
        .prepare_encrypted_push(&inputs.authority, root.path())
        .await
        .unwrap()
        .unwrap()
        .record;
    aven_conformance::tail::append_retry(
        &mut driver,
        &inputs.authority.context,
        &inputs.bearer,
        &frozen,
        publication.binding().prefix_count as i64,
    )
    .await;
}

#[test]
fn required_capabilities_are_not_skipped() {
    use aven_conformance::capability::require;
    for required in [
        Capability::ControlledClock,
        Capability::Barrier,
        Capability::LostReply,
        Capability::Restart,
    ] {
        assert_eq!(require(&[], &[required]), Err(vec![required]));
    }
}
