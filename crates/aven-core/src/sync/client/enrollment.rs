//! Repeatable device enrollment and published snapshot retrieval.
//! The public mailbox never exposes bootstrap chunks, images or credentials.
mod management;
use anyhow::{Context as _, Result, ensure};
use aven_protocol::refusal::Enrollment as Refusal;

use super::errors::is_stale;
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

pub use aven_protocol::wire::enrollment::Context;
pub use aven_protocol::wire::enrollment::{CONTROL_LIMIT, PATH, PUBLISHED_RESPONSE_LIMIT};

impl From<&ActiveInputs> for Context {
    fn from(inputs: &ActiveInputs) -> Self {
        Self {
            vault: inputs.membership.genesis().context().vault_id,
            genesis: inputs.membership.genesis().commitment(),
            device: inputs.device(),
            head: inputs.membership.head(),
        }
    }
}

pub use aven_protocol::wire::enrollment::Operation;
pub type Reply =
    aven_protocol::wire::enrollment::Reply<Evidence, Mailbox, membership::ManagementPreparation>;
/// One bounded exchange at a time; callers explicitly retry the same protected intent.
pub struct Client {
    link: Link,
    endpoint: url::Url,
    locator: String,
}
impl Client {
    pub fn new(origin: &str, link: Link) -> Result<Self> {
        ensure!(
            origin.len() <= super::MAX_SERVER_BYTES,
            "error enrollment-locator-limit"
        );
        Ok(Self {
            link,
            endpoint: super::origin::endpoint(origin, PATH)?,
            locator: origin.into(),
        })
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
        let bytes = exchange::post_json(&self.link, &self.endpoint, secret, bytes, response_limit)
            .await
            .map_err(|failure| match failure {
                exchange::Failure::Hosting(hosting) => super::errors::hosting_error(hosting),
                exchange::Failure::Network => {
                    anyhow::anyhow!("error enrollment-network outcome-unknown")
                }
                exchange::Failure::SecureTransport => {
                    anyhow::anyhow!("error enrollment-tls outcome-unknown")
                }
                exchange::Failure::Malformed => {
                    anyhow::anyhow!("error enrollment-server outcome-unknown")
                }
                exchange::Failure::TooLarge => anyhow::anyhow!("error enrollment-limit"),
                exchange::Failure::RequestBodyLimit => {
                    anyhow::anyhow!("error sync-request-body-limit")
                }
                // Only an authentication refusal says anything about this
                // device's access; timeouts and server failures stay
                // ordinary errors.
                exchange::Failure::Refused { status, code } => {
                    match Refusal::classify(status, code.as_deref()) {
                        Refusal::Stale => membership::StaleContext.into(),
                        Refusal::Busy => anyhow::anyhow!("error enrollment-busy"),
                        Refusal::Unauthorized => membership::Unauthorized.into(),
                        Refusal::Timeout => {
                            anyhow::anyhow!("error enrollment-timeout outcome-unknown")
                        }
                        Refusal::Refused => {
                            anyhow::anyhow!("error enrollment-refused outcome-unknown")
                        }
                        Refusal::Server => {
                            anyhow::anyhow!("error enrollment-server outcome-unknown")
                        }
                    }
                }
            })?;
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("error enrollment-http"))
    }
    /// Installs only a protected, independently enrolled peer into a fresh target.
    /// A completed retry is local and never rewinds later changes.
    pub async fn install(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<crate::sync::SharedStateInstallReport> {
        self.install_reporting(store, db, &|_, _| {}).await
    }

    /// [`Self::install`], reporting downloaded snapshot bytes. The total is
    /// `None` until the catalogs that size the rest have arrived.
    pub async fn install_reporting(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        downloaded: &(dyn Fn(u64, Option<u64>) + Sync),
    ) -> Result<crate::sync::SharedStateInstallReport> {
        store
            .install_peer_snapshot(db, self, &self.locator, downloaded)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn download(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        identity: [u8; 32],
        peer: &membership::Joiner,
        verified: &membership::VerifiedEnrollment,
        descriptor: &[u8],
        downloaded: &(dyn Fn(u64, Option<u64>) + Sync),
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
            head: floor.head(),
        };
        let (evidence, current) = self.membership(&initial_context, peer.bearer()).await?;
        floor = store
            .adopt_download_refresh(db, identity, peer, verified, &evidence, current)
            .await?;
        let mut retried = false;
        let mut read = async |component, index| -> Result<Vec<u8>> {
            let mut context = Context {
                vault: peer.vault(),
                genesis: verified.genesis().commitment(),
                device: peer.device(),
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
                        let (evidence, current) = self.membership(&context, peer.bearer()).await?;
                        floor = store
                            .adopt_download_refresh(
                                db, identity, peer, verified, &evidence, current,
                            )
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
        downloaded(0, None);
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
                downloaded(total, None);
                crate::sync::crash::Crash::Snapshot.at("download");
            }
        }
        let artifacts: Vec<_> =
            bootstrap_format::download::artifacts(descriptor, &package.catalogs)?
                .into_iter()
                .filter(|recipe| !matches!(recipe.component, Component::Image(_)))
                .collect();
        let expected = artifacts
            .iter()
            .flat_map(|recipe| &recipe.lengths)
            .fold(total, |sum, length| sum.saturating_add(*length));
        downloaded(total, Some(expected));
        for recipe in artifacts {
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
                downloaded(total, Some(expected));
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
    async fn membership(
        &self,
        context: &Context,
        bearer: &Secret,
    ) -> Result<(Evidence, membership::Membership)> {
        let evidence = self.unverified_membership(context, bearer).await?;
        let membership = evidence.verify()?;
        Ok((evidence, membership))
    }
    /// The caller must verify the returned chain before use.
    async fn unverified_membership(&self, context: &Context, bearer: &Secret) -> Result<Evidence> {
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
        Ok(evidence)
    }
    pub async fn refresh_inputs(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
    ) -> Result<()> {
        let (evidence, membership) = self
            .membership(&Context::active(inputs), inputs.bearer())
            .await?;
        store
            .adopt_verified_refresh(db, inputs, evidence, membership)
            .await
    }
    pub async fn refresh(&self, store: &ProtectedLocalKeyStore, db: &Database) -> Result<()> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await
    }
    #[cfg(any(test, feature = "test-support"))]
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
        let reply = retry_stale!(
            self.exchange(
                Operation::Register {
                    context: Context::active(&inputs),
                    declaration: journal.declaration.clone(),
                },
                Some(inputs.bearer()),
            )
            .await,
            self.refresh_inputs(store, db, &mut inputs).await,
        )?;
        let Reply::Registered(peer::RegistrationStatus::Open) = reply else {
            anyhow::bail!("error enrollment-invitation-unavailable");
        };
        let invitation = store.registered_invitation(db, &inputs, &journal).await?;
        let state = store
            .open_invitation(db, &inputs)
            .await?
            .context("error enrollment-invitation-missing")?;
        Ok(CreatedInvitation {
            invitation,
            state,
            vault: inputs.membership.genesis().context().vault_id,
            resumed: previous.is_some_and(|old| old.handle == journal.handle),
        })
    }
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
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
        let (record, reply) = retry_stale!(
            async {
                let record = store
                    .prepare_admission(db, &inputs, &journal, &request)
                    .await?;
                let reply = self
                    .exchange(
                        Operation::Admit {
                            context: Context::active(&inputs),
                            handle: journal.handle,
                            record: record.clone(),
                        },
                        Some(inputs.bearer()),
                    )
                    .await?;
                anyhow::Ok((record, reply))
            }
            .await,
            self.refresh_inputs(store, db, &mut inputs).await,
        )?;
        let Reply::Admitted(accepted) = reply else {
            anyhow::bail!("error enrollment-response");
        };
        ensure!(accepted == record, "error enrollment-outcome-mismatch");
        self.refresh_inputs(store, db, &mut inputs).await?;
        store.finish_inviter(db, &inputs, &journal, &record).await?;
        Ok(true)
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
                    self.finish(store, db, peer, &mail, grant).await?;
                    return Ok(true);
                }
                Ok(Reply::Mailbox(_)) => waiting = true,
                Ok(_) => {
                    failed.get_or_insert(anyhow::anyhow!("error enrollment-response"));
                }
                Err(error) if super::errors::has_code(&error, "enrollment-busy") => {
                    busy = Some(error)
                }
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
    ) -> Result<()> {
        let context = Context {
            vault: peer.vault(),
            genesis: grant.genesis,
            device: peer.device(),
            head: grant.outcome,
        };
        // `finish_peer` verifies the chain while locating the outcome.
        let evidence = self.unverified_membership(&context, peer.bearer()).await?;
        store.finish_peer(db, mail, &evidence).await
    }
}
