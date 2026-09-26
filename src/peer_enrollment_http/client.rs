use super::*;
use anyhow::ensure;

/// Enrollment exchanges with one server over HTTP. One bounded exchange at
/// a time; callers explicitly retry the same protected intent.
pub struct Client {
    pub(crate) transport: seed_bootstrap_http::Client,
    pub(crate) locator: String,
}

/// Runs one core enrollment operation over this client's transport.
macro_rules! run {
    ($self:ident, |$client:ident| $body:expr) => {
        $self
            .transport
            .driver
            .run(|link| async move {
                let $client = enrollment::Client::new(&$self.locator, link)?;
                $body.await
            })
            .await
    };
}

impl Client {
    pub fn new(origin: &str) -> Result<Self> {
        ensure!(origin.len() <= 2048, "error enrollment-locator-limit");
        let mut transport = seed_bootstrap_http::Client::new(origin)?;
        transport.endpoint.set_path(PATH);
        Ok(Self {
            transport,
            locator: origin.into(),
        })
    }
    pub(crate) async fn exchange(&self, op: Operation, secret: Option<&Secret>) -> Result<Reply> {
        run!(self, |client| client.exchange(op, secret))
    }
    /// Installs only a protected, independently enrolled peer into a fresh target.
    /// A completed retry is local and never rewinds later changes.
    pub async fn install(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<aven_core::sync::SharedStateInstallReport> {
        run!(self, |client| client.install(store, db))
    }
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        run!(self, |client| client.invite(store, db, expires))
    }
    pub async fn request(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
    ) -> Result<()> {
        run!(self, |client| client.request(store, db, invitation))
    }
    /// Requests admission with a replacement invitation from the same inviter
    /// while joining is unfinished.
    pub async fn replace(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Invitation,
    ) -> Result<()> {
        run!(self, |client| client.replace(store, db, invitation))
    }
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        run!(self, |client| client.admit(store, db))
    }
    pub async fn admit_handle(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        handle: Option<[u8; 32]>,
    ) -> Result<bool> {
        run!(self, |client| client.admit_handle(store, db, handle))
    }
    pub async fn complete(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        run!(self, |client| client.complete(store, db))
    }
    pub async fn remove_device(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: [u8; 32],
    ) -> Result<RemovalStatus> {
        run!(self, |client| client.remove_device(store, db, target))
    }
    pub(crate) async fn manage(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: Option<[u8; 32]>,
        withdraw: Option<[u8; 32]>,
    ) -> Result<RemovalStatus> {
        run!(self, |client| client.manage(store, db, target, withdraw))
    }
    pub(crate) async fn finish(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        peer: &Joiner,
        mail: &membership::Mailbox,
        grant: membership::ProvisionalGrant,
    ) -> Result<()> {
        run!(self, |client| client.finish(store, db, peer, mail, grant))
    }
    pub(crate) async fn finish_pending_management(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.finish_pending_management(store, db))
    }
    pub(crate) async fn refresh(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        run!(self, |client| client.refresh(store, db))
    }
}
