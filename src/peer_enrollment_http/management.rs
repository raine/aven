//! Removal, withdrawal of an expired possibly disclosed invitation, and one
//! bounded automatic finish, using the protected candidate owner.
use super::*;
use crate::protected_local_keys::{
    peer::{ActiveInputs, Disclosure},
    rotation::Action,
};
use aven_core::sync::seed_claim::membership::CancelStatus;

#[derive(Debug, PartialEq, Eq)]
pub enum RemovalStatus {
    Complete,
    Pending,
    /// Confirmed in-flight self-Revoke; a survivor must finish rotation.
    SelfRevoked,
}
impl Client {
    /// Remove one active device and attempt a bounded rotation for the survivors.
    /// Self-removal can confirm only its in-flight acknowledgement; retired
    /// credentials cannot resolve a lost reply. Local data and keys are retained.
    pub async fn remove_device(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: [u8; 32],
    ) -> Result<RemovalStatus> {
        Box::pin(self.manage(store, db, Some(target), None)).await
    }
    pub(crate) async fn finish_pending_management(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        self.finish_pending_removal(store, db).await?;
        Box::pin(self.withdraw_expired_disclosure(store, db)).await
    }
    /// Continues a retained removal or rotation.
    pub(crate) async fn finish_pending_removal(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        Box::pin(self.manage(store, db, None, None)).await?;
        Ok(())
    }
    async fn management_preparation(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
    ) -> Result<u64> {
        let Reply::PreparedManagement(prepared) = self
            .exchange(
                Operation::PrepareManagement {
                    context: Context::active(inputs),
                },
                Some(inputs.bearer()),
            )
            .await?
        else {
            anyhow::bail!("error management-response");
        };
        let membership = prepared.evidence.verify()?;
        ensure!(
            membership.head() == inputs.membership.head()
                && prepared.high_water >= membership.current_generation().starts_after,
            "error management-context"
        );
        store.adopt_refresh(db, inputs, prepared.evidence).await?;
        Ok(prepared.high_water)
    }
    /// Ends an expired invitation whose grant may have been sent. Admission
    /// and withdrawal are decided only from verified membership: an admitted
    /// candidate finishes `ready`; otherwise the local fence and server
    /// cancellation precede a targetless freeze and rotation, and `withdrawn`
    /// needs the chain proof. Expiry never proves withdrawal by itself.
    pub(crate) async fn withdraw_expired_disclosure(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        let handle = {
            let mut inputs = store.active_inputs(db, &self.locator).await?;
            let Some(journal) = store.expired_disclosure(db, &inputs).await? else {
                return Ok(());
            };
            self.refresh_inputs(store, db, &mut inputs).await?;
            if store.reconcile_disclosure(db, &inputs, &journal).await? != Disclosure::Unresolved {
                return Ok(());
            }
            store.mark_withdrawing(db, &inputs, &journal).await?;
            let status = self.cancel(store, db, &mut inputs, journal.handle).await?;
            self.refresh_inputs(store, db, &mut inputs).await?;
            if store.reconcile_disclosure(db, &inputs, &journal).await? != Disclosure::Unresolved {
                return Ok(());
            }
            ensure!(
                status == CancelStatus::Cancelled,
                "error withdrawal-admission-unverified"
            );
            store
                .management_intent(db, &inputs, None, Some(journal.handle))
                .await?;
            journal.handle
        };
        Box::pin(self.manage(store, db, None, Some(handle))).await?;
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        if let Some(journal) = store.expired_disclosure(db, &inputs).await? {
            store.reconcile_disclosure(db, &inputs, &journal).await?;
        }
        Ok(())
    }
    pub(crate) async fn cancel(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        inputs: &mut ActiveInputs,
        handle: [u8; 32],
    ) -> Result<CancelStatus> {
        for attempt in 0..2 {
            match self
                .exchange(
                    Operation::Cancel {
                        context: Context::active(inputs),
                        handle,
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Cancelled(status)) => return Ok(status),
                Err(error) if is_stale(&error) && attempt == 0 => {
                    self.refresh_inputs(store, db, inputs).await?
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error enrollment-response"),
            }
        }
        unreachable!()
    }
    pub(super) async fn manage(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: Option<[u8; 32]>,
        withdraw: Option<[u8; 32]>,
    ) -> Result<RemovalStatus> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let Some(intent) = store
            .management_intent(db, &inputs, target, withdraw)
            .await?
        else {
            return Ok(RemovalStatus::Complete);
        };
        let mut stale_retry = false;
        let mut rotation_attempts = 0;
        // Revoke, then Rotate, with at most one additional stale-context attempt.
        for _ in 0..3 {
            let result = self.management_preparation(store, db, &mut inputs).await;
            let high = match result {
                Ok(high) => high,
                Err(error) if is_stale(&error) && !stale_retry => {
                    stale_retry = true;
                    self.refresh_inputs(store, db, &mut inputs).await?;
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(dispatch) = store
                .management_dispatch(db, &inputs, &intent, high)
                .await?
            else {
                store
                    .management_intent(db, &inputs, target, withdraw)
                    .await?;
                return Ok(RemovalStatus::Complete);
            };
            if dispatch.action == Action::Rotate {
                rotation_attempts += 1;
                ensure!(rotation_attempts <= 2, "error management-budget");
            }
            match self
                .exchange(
                    Operation::Manage {
                        context: Context::active(&inputs),
                        record: dispatch.record.clone(),
                    },
                    Some(inputs.bearer()),
                )
                .await
            {
                Ok(Reply::Managed(record)) => {
                    ensure!(
                        record == dispatch.record,
                        "error management-outcome-mismatch"
                    );
                    if intent.target() == Some(inputs.device()) && dispatch.action == Action::Revoke
                    {
                        let result = store
                            .adopt_refresh(db, &mut inputs, dispatch.evidence)
                            .await;
                        ensure!(
                            result
                                .as_ref()
                                .err()
                                .is_some_and(|e| e.to_string() == "error enrollment-revoked"),
                            "error management-self-revoke"
                        );
                        return Ok(RemovalStatus::SelfRevoked);
                    }
                    self.refresh_inputs(store, db, &mut inputs).await?;
                    if store
                        .management_intent(db, &inputs, target, withdraw)
                        .await?
                        .is_none()
                    {
                        return Ok(RemovalStatus::Complete);
                    }
                    if dispatch.action == Action::Rotate {
                        return Ok(RemovalStatus::Pending);
                    }
                }
                Err(error) if is_stale(&error) && !stale_retry => {
                    stale_retry = true;
                    self.refresh_inputs(store, db, &mut inputs).await?;
                }
                Err(error) => return Err(error),
                _ => anyhow::bail!("error management-response"),
            }
        }
        Ok(RemovalStatus::Pending)
    }
}
