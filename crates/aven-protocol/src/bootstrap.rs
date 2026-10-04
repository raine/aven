//! Pure seed credential checks for bootstrap admission.
use crate::claim::{Genesis, Secret};
use anyhow::{Result, ensure};
/// The credential does not authenticate current membership for staging.
#[derive(Debug)]
pub struct Unauthorized;

impl std::fmt::Display for Unauthorized {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("error bootstrap-unauthorized")
    }
}

impl std::error::Error for Unauthorized {}

#[derive(Debug)]
pub struct Authentication<'a> {
    pub vault_id: [u8; 32],
    pub genesis_commitment: [u8; 32],
    pub bearer: &'a Secret,
}

/// Seed authority is checked before any stored membership or outcome state.
/// After publication the caller must also authenticate current membership.
pub fn authenticate_seed(genesis: &Genesis, auth: &Authentication<'_>) -> Result<()> {
    ensure!(
        genesis.authorizes_bearer(auth.bearer)
            && genesis.context().vault_id == auth.vault_id
            && genesis.commitment() == auth.genesis_commitment,
        Unauthorized
    );
    Ok(())
}
