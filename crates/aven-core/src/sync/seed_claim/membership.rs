//! Verified membership rules with SQLite persistence.
pub use aven_protocol::claim::membership::{
    DECLARATION_BYTES, Declaration, Device, Evidence, EvidenceRecord, Generation, Invitation,
    Joiner, MAX_CHAIN_BYTES, MAX_COVERAGE_BYTES, MAX_CUTOFF, MAX_DEVICES, MAX_EVIDENCE_JSON_BYTES,
    MAX_GENERATIONS, MAX_KEY_PLAINTEXT_BYTES, MAX_RECORD_BYTES, MAX_TRANSITIONS, Mailbox,
    Membership, ProvisionalGrant, REQUEST_BYTES, RotationMaterial, StaleContext, Unauthorized,
    VerifiedEnrollment, VerifiedKeys,
};
pub(crate) mod persistence;
pub use persistence::{CancelStatus, MAX_CANDIDATES, MAX_INVITATIONS, ManagementPreparation, now};
#[cfg(any(test, feature = "test-support"))]
pub(crate) mod test_support;
use super::*;
type Hash = [u8; 32];
fn check(condition: bool) -> Result<()> {
    ensure!(condition, "error membership-invalid");
    Ok(())
}
