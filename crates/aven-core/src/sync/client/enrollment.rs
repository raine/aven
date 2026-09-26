//! Repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
mod management;
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};

use super::exchange::{self, Link};
use super::keys::ProtectedLocalKeyStore;
use super::keys::peer::{ActiveInputs, OpenInvitation};
use crate::db::Database;
use crate::sync::seed_claim::{
    Secret,
    membership::{self, Evidence, Invitation, Mailbox},
    peer,
};
pub use management::RemovalStatus;

pub struct CreatedInvitation {
    pub invitation: Invitation,
    pub state: OpenInvitation,
    pub vault: [u8; 32],
    pub resumed: bool,
}

pub const PATH: &str = "/e2ee/enrollment/v1";
const BUSY_RETRIES: usize = 3;
pub const CONTROL_LIMIT: usize =
    crate::sync::base64_bytes::encoded_len(membership::MAX_RECORD_BYTES) + 4096;
pub const PUBLISHED_RESPONSE_LIMIT: usize =
    crate::sync::base64_bytes::encoded_len(crate::sync::bootstrap_staging::MAX_REQUEST_BYTES)
        + 4096;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub vault: [u8; 32],
    pub genesis: [u8; 32],
    pub device: [u8; 32],
    pub credential_version: u32,
    pub head: [u8; 32],
}
impl Context {
    pub fn active(inputs: &ActiveInputs) -> Self {
        Self {
            vault: inputs.membership.genesis().context().vault_id,
            genesis: inputs.membership.genesis().commitment(),
            device: inputs.device(),
            credential_version: 1,
            head: inputs.membership.head(),
        }
    }
    pub fn auth<'a>(&self, bearer: &'a Secret) -> peer::Authentication<'a> {
        peer::Authentication {
            vault: self.vault,
            genesis: self.genesis,
            device: self.device,
            credential_version: self.credential_version,
            head: self.head,
            bearer,
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Operation {
    Register {
        context: Context,
        #[serde(with = "crate::sync::base64_bytes")]
        declaration: Vec<u8>,
    },
    Post {
        vault: [u8; 32],
        handle: [u8; 32],
        #[serde(with = "crate::sync::base64_bytes")]
        request: Vec<u8>,
    },
    Mailbox {
        vault: [u8; 32],
        handle: [u8; 32],
    },
    Admit {
        context: Context,
        handle: [u8; 32],
        #[serde(with = "crate::sync::base64_bytes")]
        record: Vec<u8>,
    },
    Membership {
        context: Context,
    },
    PrepareManagement {
        context: Context,
    },
    Manage {
        context: Context,
        #[serde(with = "crate::sync::base64_bytes")]
        record: Vec<u8>,
    },
    Cancel {
        context: Context,
        handle: [u8; 32],
    },
    Published {
        context: Context,
        descriptor: [u8; 32],
        component: Option<crate::sync::bootstrap_staging::Component>,
        index: u64,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Reply {
    Done,
    Registered(peer::RegistrationStatus),
    Mailbox(Mailbox),
    Admitted(#[serde(with = "crate::sync::base64_bytes")] Vec<u8>),
    Membership(Evidence),
    PreparedManagement(membership::ManagementPreparation),
    Managed(#[serde(with = "crate::sync::base64_bytes")] Vec<u8>),
    Cancelled(membership::CancelStatus),
    Published(#[serde(with = "crate::sync::base64_bytes")] Vec<u8>),
}
/// One bounded exchange at a time; callers explicitly retry the same protected intent.
pub struct Client {
    link: Link,
    endpoint: url::Url,
    locator: String,
}
impl Client {
    pub fn new(origin: &str, link: Link) -> Result<Self> {
        ensure!(origin.len() <= 2048, "error enrollment-locator-limit");
        Ok(Self {
            link,
            endpoint: super::origin::endpoint(origin, PATH)?,
            locator: origin.into(),
        })
    }
    pub fn locator(&self) -> &str {
        &self.locator
    }
    pub(crate) fn link(&self) -> &Link {
        &self.link
    }
    pub async fn exchange(&self, op: Operation, secret: Option<&Secret>) -> Result<Reply> {
        let response_limit = if matches!(&op, Operation::Published { .. }) {
            PUBLISHED_RESPONSE_LIMIT
        } else if matches!(&op, Operation::PrepareManagement { .. }) {
            membership::MAX_EVIDENCE_JSON_BYTES + 128
        } else if matches!(&op, Operation::Membership { .. }) {
            membership::MAX_EVIDENCE_JSON_BYTES
        } else {
            CONTROL_LIMIT
        };
        let bytes =
            serde_json::to_vec(&op).map_err(|_| anyhow::anyhow!("error enrollment-http"))?;
        ensure!(bytes.len() <= CONTROL_LIMIT, "error enrollment-limit");
        let mut headers = vec![exchange::json_content()];
        if let Some(secret) = secret {
            headers.push(exchange::bearer(secret));
        }
        let mut attempt = 0;
        let response = loop {
            let response = self
                .link
                .send(
                    "POST",
                    &self.endpoint,
                    headers.clone(),
                    bytes.clone(),
                    response_limit,
                )
                .await
                .map_err(|_| anyhow::anyhow!("error enrollment-network outcome-unknown"))?;
            if response.status == 200 {
                break response;
            }
            let status = response.status;
            let retry_after = response.retry_after();
            ensure!(
                response.body.len() <= 256,
                "error enrollment-server outcome-unknown"
            );
            let response_bytes = response.body;
            let busy =
                status == 503 && response_bytes == b"enrollment-busy" && retry_after.is_some();
            if busy && attempt < BUSY_RETRIES {
                self.link
                    .wait(exchange::busy_retry_delay(attempt, retry_after.unwrap()))
                    .await;
                attempt += 1;
                continue;
            }
            if status == 409 && response_bytes == b"membership-stale" {
                anyhow::bail!(membership::StaleContext);
            }
            if busy {
                anyhow::bail!("error enrollment-busy");
            }
            // Only an authentication refusal says anything about this device's
            // access; timeouts and server failures stay ordinary errors.
            match (status, response_bytes.as_slice()) {
                (403, b"enrollment-unauthorized") => {
                    anyhow::bail!("error enrollment-unauthorized")
                }
                (400, b"enrollment-refused") => {
                    anyhow::bail!("error enrollment-refused outcome-unknown")
                }
                (408, _) => {
                    anyhow::bail!("error enrollment-timeout outcome-unknown")
                }
                _ => anyhow::bail!("error enrollment-server outcome-unknown"),
            }
        };
        ensure!(
            response.is_json() && !response.has_header("content-encoding"),
            "error enrollment-server outcome-unknown"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|n| n <= response_limit as u64),
            "error enrollment-limit"
        );
        ensure!(
            response.body.len() <= response_limit,
            "error enrollment-limit"
        );
        let bytes = response.body;
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error enrollment-http"))
    }
    /// Installs only a protected, independently enrolled peer into a fresh target.
    /// A completed retry is local and never rewinds later changes.
    pub async fn install(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<crate::sync::SharedStateInstallReport> {
        store.install_peer_snapshot(db, self, &self.locator).await
    }

    pub(crate) async fn download(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        identity: [u8; 32],
        peer: &membership::Joiner,
        verified: &membership::VerifiedEnrollment,
        descriptor: &[u8],
    ) -> Result<crate::sync::bootstrap_format::download::Metadata> {
        use crate::sync::{
            bootstrap_format::{self, download::Metadata},
            bootstrap_staging::{Component, MAX_CHUNKS, MAX_STORAGE_BYTES},
        };
        let b = verified.publication().binding();
        let mut floor = verified.membership().clone();
        let initial_context = Context {
            vault: peer.vault(),
            genesis: verified.genesis().commitment(),
            device: peer.device(),
            credential_version: 1,
            head: floor.head(),
        };
        let evidence = self.membership(&initial_context, peer.bearer()).await?;
        floor = store
            .adopt_download_refresh(db, identity, peer, verified, &evidence)
            .await?;
        let mut retried = false;
        let mut read = async |component, index| -> Result<Vec<u8>> {
            let mut context = Context {
                vault: peer.vault(),
                genesis: verified.genesis().commitment(),
                device: peer.device(),
                credential_version: 1,
                head: floor.head(),
            };
            loop {
                match self
                    .exchange(
                        Operation::Published {
                            context: context.clone(),
                            descriptor: b.descriptor_commitment,
                            component,
                            index,
                        },
                        Some(peer.bearer()),
                    )
                    .await
                {
                    Ok(Reply::Published(bytes)) => return Ok(bytes),
                    Err(error) if is_stale(&error) && !retried => {
                        retried = true;
                        let evidence = self.membership(&context, peer.bearer()).await?;
                        floor = store
                            .adopt_download_refresh(db, identity, peer, verified, &evidence)
                            .await?;
                        context.head = floor.head();
                    }
                    Err(error) => return Err(error),
                    _ => anyhow::bail!("error snapshot-response"),
                }
            }
        };
        ensure!(
            read(None, 0).await? == descriptor,
            "error snapshot-descriptor-substitution"
        );
        let mut package = Metadata {
            descriptor: descriptor.to_vec(),
            catalogs: Default::default(),
            state: vec![],
            manifest: vec![],
        };
        let mut total = 0_u64;
        let mut chunks = 0_u64;
        for (i, recipe) in bootstrap_format::download::catalogs(descriptor)?
            .into_iter()
            .enumerate()
        {
            for (index, length) in recipe.lengths.into_iter().enumerate() {
                let bytes = read(Some(recipe.component), u64::try_from(index)?).await?;
                ensure!(
                    u64::try_from(bytes.len())? == length,
                    "error snapshot-length"
                );
                total = total
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("error snapshot-limit"))?;
                chunks += 1;
                ensure!(
                    total <= MAX_STORAGE_BYTES && chunks <= MAX_CHUNKS,
                    "error snapshot-limit"
                );
                package.catalogs[i].extend(bytes);
                #[cfg(any(test, feature = "test-support"))]
                if std::env::var("AVEN_SNAPSHOT_CRASH").as_deref() == Ok("download") {
                    std::process::exit(83);
                }
            }
        }
        for recipe in bootstrap_format::download::artifacts(descriptor, &package.catalogs)? {
            if matches!(recipe.component, Component::Image(_)) {
                continue;
            }
            let mut records = Vec::new();
            for (index, length) in recipe.lengths.into_iter().enumerate() {
                let bytes = read(Some(recipe.component), u64::try_from(index)?).await?;
                ensure!(
                    u64::try_from(bytes.len())? == length,
                    "error snapshot-length"
                );
                total = total
                    .checked_add(length)
                    .ok_or_else(|| anyhow::anyhow!("error snapshot-limit"))?;
                chunks += 1;
                ensure!(
                    total <= MAX_STORAGE_BYTES && chunks <= MAX_CHUNKS,
                    "error snapshot-limit"
                );
                records.push(bytes);
            }
            match recipe.component {
                Component::Manifest => package.manifest = records,
                Component::State => package.state = records,
                _ => anyhow::bail!("error snapshot-component"),
            }
        }
        Ok(package)
    }
    async fn membership(&self, context: &Context, bearer: &Secret) -> Result<Evidence> {
        let Reply::Membership(evidence) = self
            .exchange(
                Operation::Membership {
                    context: context.clone(),
                },
                Some(bearer),
            )
            .await?
        else {
            anyhow::bail!("error membership-response");
        };
        evidence.verify()?;
        Ok(evidence)
    }
    pub async fn refresh_inputs(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
    ) -> Result<()> {
        let evidence = self
            .membership(&Context::active(inputs), inputs.bearer())
            .await?;
        store.adopt_refresh(db, inputs, evidence).await
    }
    pub async fn refresh(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<()> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await
    }
    pub async fn invite(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<Invitation> {
        Ok(self
            .invite_with_status(store, db, expires)
            .await?
            .invitation)
    }

    pub async fn invite_with_status(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        expires: u64,
    ) -> Result<CreatedInvitation> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let previous = store.open_invitation(db, &inputs).await?;
        let journal = store
            .prepare_invitation(db, &inputs, Some(expires), None)
            .await?;
        for attempt in 0..2 {
            match self
                .exchange(
                    Operation::Register {
                        context: Context::active(&inputs),
                        declaration: journal.declaration.clone(),
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Registered(peer::RegistrationStatus::Open)) => {
                    let invitation = store.registered_invitation(db, &inputs, &journal).await?;
                    let state = store
                        .open_invitation(db, &inputs)
                        .await?
                        .context("error enrollment-invitation-missing")?;
                    return Ok(CreatedInvitation {
                        invitation,
                        state,
                        vault: inputs.membership.genesis().context().vault_id,
                        resumed: previous.is_some_and(|old| old.handle == journal.handle),
                    });
                }
                Err(error) if is_stale(&error) && attempt == 0 => {
                    self.refresh_inputs(store, db, &mut inputs).await?
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error enrollment-invitation-unavailable"),
            }
        }
        unreachable!()
    }
    pub async fn request(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
    ) -> Result<()> {
        let peer = self.prepare(store, db, invitation, false).await?;
        self.post(&peer).await
    }
    /// Requests admission with a replacement invitation from the same inviter
    /// while joining is unfinished. Earlier attempts stay retained and can
    /// still complete the join.
    pub async fn replace(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Invitation,
    ) -> Result<()> {
        let peer = self.prepare(store, db, Some(invitation), true).await?;
        self.post(&peer).await
    }
    /// Selects or creates the attempt to post from protected local state.
    /// Its failures are integrity or eligibility refusals, never outcomes of
    /// an exchange.
    pub async fn prepare(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        invitation: Option<Invitation>,
        replace: bool,
    ) -> Result<membership::Joiner> {
        match invitation {
            Some(invitation) if replace => store.replace_peer(db, &self.locator, invitation).await,
            invitation => store.prepare_peer(db, &self.locator, invitation).await,
        }
    }
    /// Whether attempts other than the one `prepare` returned could still
    /// complete this join.
    pub async fn has_other_attempts(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<bool> {
        Ok(store.peer_attempts(db, &self.locator).await?.len() > 1)
    }
    pub async fn post(&self, peer: &membership::Joiner) -> Result<()> {
        ensure!(
            matches!(
                self.exchange(
                    Operation::Post {
                        vault: peer.vault(),
                        handle: peer.handle(),
                        request: peer.request().to_vec()
                    },
                    None
                )
                .await?,
                Reply::Done
            ),
            "error enrollment-response"
        );
        Ok(())
    }
    pub async fn admit(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        self.admit_handle(store, db, None).await
    }
    /// Whether a device has asked to join with this invitation. Only the
    /// server is consulted, so frequent checks stay cheap.
    pub async fn join_requested(&self, vault: [u8; 32], handle: [u8; 32]) -> Result<bool> {
        match self
            .exchange(Operation::Mailbox { vault, handle }, None)
            .await?
        {
            Reply::Mailbox(mail) => Ok(mail.request.is_some()),
            _ => anyhow::bail!("error enrollment-response"),
        }
    }
    /// Exact historical outcomes remain addressable after subsequent invitations.
    pub async fn admit_handle(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        handle: Option<[u8; 32]>,
    ) -> Result<bool> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let journal = store.prepare_invitation(db, &inputs, None, handle).await?;
        let Reply::Mailbox(mail) = self
            .exchange(
                Operation::Mailbox {
                    vault: inputs.membership.genesis().context().vault_id,
                    handle: journal.handle,
                },
                None,
            )
            .await?
        else {
            anyhow::bail!("error enrollment-response");
        };
        let Some(request) = mail.request else {
            return Ok(false);
        };
        for attempt in 0..2 {
            let record = store
                .prepare_admission(db, &inputs, &journal, &request)
                .await?;
            match self
                .exchange(
                    Operation::Admit {
                        context: Context::active(&inputs),
                        handle: journal.handle,
                        record: record.clone(),
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Admitted(accepted)) => {
                    ensure!(accepted == record, "error enrollment-outcome-mismatch");
                    self.refresh_inputs(store, db, &mut inputs).await?;
                    store.finish_inviter(db, &inputs, &journal, &record).await?;
                    return Ok(true);
                }
                Err(error) if is_stale(&error) && attempt == 0 => {
                    self.refresh_inputs(store, db, &mut inputs).await?
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error enrollment-response"),
            }
        }
        unreachable!()
    }
    /// Checks every retained attempt for an admission and completes the one
    /// whose grant opens for its exact request; a grant that does not open is
    /// an integrity failure. A refusal or unreachable mailbox for one attempt
    /// proves nothing about the others: any busy mailbox is reported as busy,
    /// any attempt still waiting yields `false`, and only when every mailbox
    /// failed is the latest attempt's error reported. Once a response is
    /// pinned, only that attempt is checked.
    pub async fn complete(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<bool> {
        let attempts = store.peer_attempts(db, &self.locator).await?;
        let (mut busy, mut waiting, mut failed) = (None, false, None);
        for peer in attempts.iter().rev() {
            match self
                .exchange(
                    Operation::Mailbox {
                        vault: peer.vault(),
                        handle: peer.handle(),
                    },
                    None,
                )
                .await
            {
                Ok(Reply::Mailbox(mail)) if mail.admission.is_some() => {
                    let grant = store.open_peer_response(db, &mail).await?;
                    return self.finish(store, db, peer, &mail, grant).await;
                }
                Ok(Reply::Mailbox(_)) => waiting = true,
                Ok(_) => {
                    failed.get_or_insert(anyhow::anyhow!("error enrollment-response"));
                }
                Err(error) if error.to_string() == "error enrollment-busy" => busy = Some(error),
                Err(error) => {
                    failed.get_or_insert(error);
                }
            }
        }
        match (busy, failed) {
            (Some(error), _) => Err(error),
            _ if waiting => Ok(false),
            (None, Some(error)) => Err(error),
            (None, None) => Ok(false),
        }
    }
    pub async fn finish(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        peer: &membership::Joiner,
        mail: &membership::Mailbox,
        grant: membership::ProvisionalGrant,
    ) -> Result<bool> {
        let context = Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            credential_version: 1,
            head: grant.outcome,
        };
        let evidence = self.membership(&context, peer.bearer()).await?;
        store.finish_peer(db, mail, &evidence).await?;
        Ok(true)
    }
}

pub fn is_stale(error: &anyhow::Error) -> bool {
    error.is::<membership::StaleContext>()
}
