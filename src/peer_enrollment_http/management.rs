//! Removal and one bounded automatic finish, using the protected candidate owner.
use super::*;
use crate::protected_local_keys::{peer::ActiveInputs, rotation::Action};

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
        Box::pin(self.manage(store, db, Some(target))).await
    }
    pub(crate) async fn finish_pending_management(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
    ) -> Result<()> {
        Box::pin(self.manage(store, db, None)).await?;
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
    async fn manage(
        &self,
        store: &ProtectedLocalKeyStore,
        db: &Database,
        target: Option<[u8; 32]>,
    ) -> Result<RemovalStatus> {
        let mut inputs = store.active_inputs(db, &self.locator).await?;
        self.refresh_inputs(store, db, &mut inputs).await?;
        let Some(intent) = store.management_intent(db, &inputs, target).await? else {
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
                store.management_intent(db, &inputs, target).await?;
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
                        .management_intent(db, &inputs, target)
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
