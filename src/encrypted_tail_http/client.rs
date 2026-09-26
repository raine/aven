use super::*;
use aven_core::sync::client::tail as tail_client;
use std::path::Path;

/// Encrypted tail exchanges with one server over HTTP.
pub struct Client {
    pub(crate) transport: seed_bootstrap_http::Client,
    pub(crate) locator: String,
}

/// Runs one core tail operation over this client's transport.
macro_rules! run {
    ($self:ident, |$client:ident| $body:expr) => {
        $self
            .transport
            .driver
            .run(|link| async move {
                let $client = tail_client::Client::new(&$self.locator, link)?;
                $body.await
            })
            .await
    };
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        let mut transport = seed_bootstrap_http::Client::new(origin)?;
        transport.endpoint.set_path(PATH);
        Ok(Self {
            transport,
            locator: origin.into(),
        })
    }
    pub(crate) async fn exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: Operation,
    ) -> Result<Reply> {
        run!(self, |client| client.exchange(context, bearer, operation))
    }
    pub(crate) async fn image_exchange(
        &self,
        context: &Context,
        bearer: &Secret,
        operation: aven_core::sync::encrypted_tail::attachments::Operation,
    ) -> Result<aven_core::sync::encrypted_tail::attachments::Reply> {
        run!(self, |client| client
            .image_exchange(context, bearer, operation))
    }
    pub(crate) async fn push(
        &self,
        inputs: &TailSnapshot,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<PushStep> {
        run!(self, |client| client.push(inputs, db, blob_dir))
    }
    /// Reads one authorized page without preparing uploads.
    pub async fn pull_only_round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        run!(self, |client| client.pull_only_round(store, db))
    }
    /// Resolves at most one ordered local head, applies one metadata page and
    /// downloads at most one image.
    pub async fn round(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
    ) -> Result<Round> {
        run!(self, |client| client.round(store, db, blob_dir))
    }
    /// Repairs one known reference without changing its descriptor or metadata.
    pub async fn repair_attachment(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        workspace: &str,
        reference: &str,
    ) -> Result<()> {
        run!(self, |client| client
            .repair_attachment(store, db, blob_dir, workspace, reference))
    }
    pub(crate) async fn start_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<DrainSnapshot> {
        run!(self, |client| client.start_drain(store, db))
    }
    pub(crate) async fn round_in_drain(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        blob_dir: &Path,
        drain: &mut DrainSnapshot,
    ) -> Result<Round> {
        run!(self, |client| client
            .round_in_drain(store, db, blob_dir, drain))
    }
}
