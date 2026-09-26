//! Bounded append-only removal intents and exact management dispatch ownership.
use super::{membership::EvidenceRef, peer::ActiveInputs, *};
use crate::sync::seed_claim::membership::{
    Evidence, MAX_CANDIDATES, MAX_RECORD_BYTES, MAX_TRANSITIONS, Membership, RotationMaterial,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
type Hash = [u8; 32];
const INTENT_BYTES: usize = 1024;
const PLAN_BYTES: usize = 1024;
const RECORD_BYTES: usize = MAX_RECORD_BYTES + 128;
const MATERIAL_BYTES: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    index: usize,
    target: Option<Hash>,
    /// Outbound invitation handle whose possibly sent grant this freeze and
    /// rotation withdraw. Removal intents carry `target` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    withdraw: Option<Hash>,
    original: EvidenceRef,
}
impl Intent {
    fn name(&self, phase: &str) -> String {
        format!("management-{}-{phase}", self.index)
    }
    pub fn target(&self) -> Option<Hash> {
        self.target
    }
    /// Removal revokes its target; withdrawal freezes without one.
    fn revoke_targets(&self) -> Result<Vec<Hash>> {
        match (self.target, self.withdraw) {
            (Some(target), None) => Ok(vec![target]),
            (None, Some(_)) => Ok(vec![]),
            _ => anyhow::bail!("error management-target"),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    Revoke,
    Rotate,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    before: EvidenceRef,
    action: Action,
    cutoff: u64,
}
pub struct Dispatch {
    pub record: Vec<u8>,
    pub evidence: Evidence,
    pub membership: Membership,
    pub action: Action,
}

impl ProtectedLocalKeyStore {
    fn management_action(&self, inputs: &ActiveInputs, intent: &Intent) -> Result<Option<Action>> {
        let (_, original) = self.load_evidence(&intent.original)?;
        ensure!(
            inputs.membership.extends(&original),
            "error management-fork"
        );
        match (intent.target, intent.withdraw) {
            (Some(target), None) => {
                ensure!(original.has_device(target), "error management-target")
            }
            (None, None) => ensure!(original.rotation_pending(), "error management-intent"),
            (None, Some(_)) => {}
            (Some(_), Some(_)) => anyhow::bail!("error management-intent"),
        }
        let mut removed_generation = intent
            .target
            .is_none()
            .then_some(original.current_generation().id);
        let mut before = original.clone();
        for t in inputs
            .evidence
            .transitions
            .iter()
            .skip(original.sequence() as usize - 1)
        {
            let next = before.append(&t.declaration, &t.request, &t.record)?;
            if removed_generation.is_none()
                && intent.target.is_some_and(|target| !next.has_device(target))
            {
                removed_generation = Some(next.current_generation().id);
            }
            if removed_generation
                .is_some_and(|generation| generation != next.current_generation().id)
                && !next.rotation_pending()
            {
                return Ok(None);
            }
            before = next;
        }
        // Withdrawal freezes unless membership is already pending.
        let freeze = intent.withdraw.is_some() && !inputs.membership.rotation_pending();
        Ok(Some(if removed_generation.is_none() || freeze {
            Action::Revoke
        } else {
            Action::Rotate
        }))
    }

    /// Validate every retained phase before treating its slot as committed or
    /// lost. Without `prove`, only phase presence, framing and evidence digests
    /// are checked, for an intent whose completion `ready` already proves.
    async fn management_plans(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        intent: &Intent,
        prove: bool,
    ) -> Result<Vec<(Plan, Option<Membership>, Option<Vec<u8>>)>> {
        let mut plans = Vec::new();
        let mut gap = false;
        let mut previous_sequence = 0;
        for index in 0..MAX_CANDIDATES {
            let raw = self
                .phase(db, &intent.name(&format!("plan-{index}")), PLAN_BYTES)
                .await?;
            let record = self
                .phase(
                    db,
                    &intent.name(&format!("candidate-{index}")),
                    RECORD_BYTES,
                )
                .await?;
            let material = self
                .phase(
                    db,
                    &intent.name(&format!("material-{index}")),
                    MATERIAL_BYTES,
                )
                .await?;
            let sent = self
                .phase(db, &intent.name(&format!("sent-{index}")), 128)
                .await?;
            let Some(raw) = raw else {
                ensure!(
                    record.is_none() && material.is_none() && sent.is_none(),
                    "error management-plan-missing"
                );
                gap = true;
                continue;
            };
            ensure!(!gap, "error management-plan-missing");
            let plan: Plan = serde_json::from_slice(&raw)?;
            if plan.action == Action::Revoke {
                ensure!(
                    material.is_none() && plan.cutoff == 0,
                    "error management-plan"
                );
            }
            if let Some(material) = &material {
                RotationMaterial::from_protected_storage(material)?;
            }
            ensure!(
                record.is_some() || sent.is_none(),
                "error management-candidate-missing"
            );
            ensure!(
                record.is_none() || plan.action != Action::Rotate || material.is_some(),
                "error management-material-missing"
            );
            if !prove {
                self.load_evidence_bytes(&plan.before)?;
                plans.push((plan, None, record.map(|r| r.to_vec())));
                continue;
            }
            let (_, before) = self.load_evidence(&plan.before)?;
            ensure!(inputs.membership.extends(&before), "error management-fork");
            ensure!(
                before.sequence() > previous_sequence,
                "error management-plan-order"
            );
            previous_sequence = before.sequence();
            if let Some(record) = &record {
                let after = before.append(&[], &[], record)?;
                if plan.action == Action::Revoke {
                    ensure!(
                        inputs
                            .authority()
                            .prepare_revoke(&before, &intent.revoke_targets()?)?
                            == record.as_slice(),
                        "error management-target-mismatch"
                    );
                }
                ensure!(
                    (after.generations().len() > before.generations().len())
                        == (plan.action == Action::Rotate),
                    "error management-action"
                );
                if plan.action == Action::Rotate {
                    ensure!(
                        after.current_generation().starts_after == plan.cutoff,
                        "error management-cutoff"
                    );
                    RotationMaterial::from_protected_storage(
                        material
                            .as_ref()
                            .context("error management-material-missing")?,
                    )?
                    .validate_generation(&after)?;
                }
                if inputs.membership.head_at(after.sequence()) == Some(after.head()) {
                    ensure!(sent.is_some(), "error management-sent-missing");
                }
                if let Some(sent) = &sent {
                    ensure!(
                        sent.as_slice() == after.head(),
                        "error management-sent-mismatch"
                    );
                }
            }
            plans.push((plan, Some(before), record.map(|r| r.to_vec())));
        }
        Ok(plans)
    }
    /// `target` requests removal, `withdraw` an invitation withdrawal, and
    /// neither resumes unfinished work or finishes a pending rotation.
    pub async fn management_intent(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        target: Option<Hash>,
        withdraw: Option<Hash>,
    ) -> Result<Option<Intent>> {
        let mut next = 0;
        let mut gap = false;
        let mut unfinished = None;
        let mut completed_target = false;
        for index in 0..MAX_TRANSITIONS {
            let raw = self
                .phase(db, &format!("management-{index}-intent"), INTENT_BYTES)
                .await?;
            let Some(raw) = raw else {
                gap = true;
                continue;
            };
            ensure!(!gap, "error management-intent-missing");
            ensure!(unfinished.is_none(), "error management-intent-order");
            next = index + 1;
            let intent: Intent = serde_json::from_slice(&raw)?;
            ensure!(intent.index == index, "error management-intent");
            // `ready` was recorded only once the chain proved the intent done,
            // and every extension of that head stays done, so a completed
            // intent keeps only its structural phase checks.
            let done = if let Some(ready) = self.phase(db, &intent.name("ready"), 128).await? {
                ensure!(
                    inputs
                        .membership
                        .contains_head(&ready.as_slice().try_into()?),
                    "error management-ready-mismatch"
                );
                self.load_evidence_bytes(&intent.original)?;
                self.management_plans(db, inputs, &intent, false).await?;
                true
            } else {
                self.management_plans(db, inputs, &intent, true).await?;
                let done = self.management_action(inputs, &intent)?.is_none();
                if done {
                    self.save_management_phase(
                        db,
                        inputs,
                        &intent.name("ready"),
                        128,
                        &inputs.membership.head(),
                    )
                    .await?;
                }
                done
            };
            if done {
                if (target.is_some() && target == intent.target)
                    || (withdraw.is_some() && withdraw == intent.withdraw)
                {
                    completed_target = true;
                }
            } else {
                unfinished = Some(intent);
            }
        }
        if completed_target {
            return Ok(None);
        }
        if let Some(intent) = unfinished {
            ensure!(
                (target.is_none() && withdraw.is_none())
                    || (target == intent.target && withdraw == intent.withdraw),
                "error management-unfinished"
            );
            return Ok(Some(intent));
        }
        if target.is_none() && withdraw.is_none() && !inputs.membership.rotation_pending() {
            return Ok(None);
        }
        ensure!(next < MAX_TRANSITIONS, "error membership-change-limit");
        if let Some(target) = target {
            inputs
                .authority()
                .prepare_revoke(&inputs.membership, &[target])?;
        }
        let intent = Intent {
            index: next,
            target,
            withdraw,
            original: self.save_evidence(&inputs.evidence)?,
        };
        self.save_management_phase(
            db,
            inputs,
            &intent.name("intent"),
            INTENT_BYTES,
            &serde_json::to_vec(&intent)?,
        )
        .await?;
        Ok(Some(intent))
    }
    pub async fn management_dispatch(
        &self,
        db: &Database,
        inputs: &ActiveInputs,
        intent: &Intent,
        high: u64,
    ) -> Result<Option<Dispatch>> {
        let plans = self.management_plans(db, inputs, intent, true).await?;
        let Some(action) = self.management_action(inputs, intent)? else {
            return Ok(None);
        };
        let mut index = plans.len();
        let mut selected = None;
        for (n, (plan, before, record)) in plans.into_iter().enumerate() {
            let before = before.context("error management-plan")?;
            if let Some(head) = inputs.membership.head_at(before.sequence() + 1) {
                if record
                    .as_ref()
                    .is_some_and(|r| Sha256::digest(r).as_slice() == head)
                {
                    ensure!(
                        self.phase(db, &intent.name(&format!("sent-{n}")), 128)
                            .await?
                            .is_some(),
                        "error management-sent-missing"
                    );
                }
                continue;
            }
            ensure!(
                before.head() == inputs.membership.head() && plan.action == action,
                "error management-unresolved"
            );
            index = n;
            selected = Some((plan, record));
            break;
        }
        ensure!(index < MAX_CANDIDATES, "error management-limit");
        let (plan, saved) = match selected {
            Some(value) => value,
            None => {
                let plan = Plan {
                    before: self.save_evidence(&inputs.evidence)?,
                    action,
                    cutoff: if action == Action::Rotate { high } else { 0 },
                };
                self.save_management_phase(
                    db,
                    inputs,
                    &intent.name(&format!("plan-{index}")),
                    PLAN_BYTES,
                    &serde_json::to_vec(&plan)?,
                )
                .await?;
                (plan, None)
            }
        };
        let record = if let Some(saved) = saved {
            saved
        } else {
            let record = match action {
                Action::Revoke => inputs
                    .authority()
                    .prepare_revoke(&inputs.membership, &intent.revoke_targets()?)?,
                Action::Rotate => {
                    ensure!(plan.cutoff == high, "error management-cutoff");
                    let name = intent.name(&format!("material-{index}"));
                    let material = match self.phase(db, &name, MATERIAL_BYTES).await? {
                        Some(bytes) => RotationMaterial::from_protected_storage(&bytes)?,
                        None => {
                            let material = RotationMaterial::generate()?;
                            self.save_management_phase(
                                db,
                                inputs,
                                &name,
                                MATERIAL_BYTES,
                                &material.protected_storage_bytes(),
                            )
                            .await?;
                            material
                        }
                    };
                    inputs.authority().prepare_rotation_with(
                        &inputs.membership,
                        inputs.generation_keys(),
                        plan.cutoff,
                        &material,
                    )?
                }
            };
            self.save_management_phase(
                db,
                inputs,
                &intent.name(&format!("candidate-{index}")),
                RECORD_BYTES,
                &record,
            )
            .await?;
            record
        };
        self.save_management_phase(
            db,
            inputs,
            &intent.name(&format!("sent-{index}")),
            128,
            &Sha256::digest(&record),
        )
        .await?;
        let (mut evidence, before) = self.load_evidence(&plan.before)?;
        let membership = before.append(&[], &[], &record)?;
        evidence
            .transitions
            .push(crate::sync::seed_claim::membership::EvidenceRecord {
                declaration: vec![],
                request: vec![],
                record: record.clone(),
            });
        Ok(Some(Dispatch {
            record,
            evidence,
            membership,
            action,
        }))
    }
}
