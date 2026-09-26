use super::*;
use aven_core::sync::client::bootstrap;

/// Seed bootstrap exchanges with one server over HTTP.
pub struct Client {
    pub(crate) http: reqwest::Client,
    pub(crate) endpoint: reqwest::Url,
    pub(crate) driver: HttpDriver,
    pub(crate) origin: String,
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        let driver = HttpDriver::new()?;
        let _ = bootstrap::Client::new(origin, Default::default())?;
        let mut endpoint =
            reqwest::Url::parse(origin).map_err(|_| anyhow::anyhow!("error bootstrap-origin"))?;
        endpoint.set_path(PATH);
        Ok(Self {
            http: driver.http.clone(),
            endpoint,
            driver,
            origin: origin.into(),
        })
    }
    pub(crate) async fn exchange(
        &self,
        genesis: &Genesis,
        secret: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .exchange(genesis, secret, operation)
                    .await
            })
            .await
    }

    /// Initial setup claim or exact bearer-authorized claim resumption.
    pub async fn claim(
        &self,
        genesis: &Genesis,
        authentication: ClaimAuthentication<'_>,
    ) -> Result<()> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .claim(genesis, authentication)
                    .await
            })
            .await
    }

    /// Resume one frozen candidate, then validate outcome, adopt and clean up.
    pub async fn resume(
        &self,
        store: &ProtectedLocalKeyStore,
        database: &Database,
    ) -> Result<bool> {
        self.driver
            .run(|link| async move {
                bootstrap::Client::new(&self.origin, link)?
                    .resume(store, database)
                    .await
            })
            .await
    }
}
