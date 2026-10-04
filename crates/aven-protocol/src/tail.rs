//! Current membership and generation rules for ordinary tail and image admission.
//! Storage owners establish prefix identities, saved outcomes, parent state,
//! attachment completeness and allocator ordering before committing effects.
use crate::claim::{Secret, membership::Membership};
use crate::wire::tail::Context;
use anyhow::{Result, ensure};

/// Authenticate current device authority before checking the publication binding.
pub fn authenticate(membership: &Membership, context: &Context, bearer: &Secret) -> Result<()> {
    membership.authenticate(&context.authentication(bearer), false)?;
    let binding = membership.publication().binding();
    ensure!(
        context.stream == binding.stream_id && context.descriptor == binding.descriptor_commitment,
        "error encrypted-tail-invalid"
    );
    Ok(())
}

/// Only fresh appends require an unfrozen current generation. Saved outcomes
/// must be resolved first under current authentication, including after rotation.
pub fn admit_generations(
    membership: &Membership,
    generations: impl IntoIterator<Item = [u8; 32]>,
) -> Result<()> {
    ensure!(
        !membership.rotation_pending(),
        "error membership-rotation-pending"
    );
    ensure!(
        generations
            .into_iter()
            .all(|generation| generation == membership.current_generation().id),
        "error encrypted-tail-invalid"
    );
    Ok(())
}

/// Historical-generation repair requires stored provenance for this exact
/// descriptor. A known generation alone does not admit a new historical object.
pub fn eligible_image(membership: &Membership, generation: [u8; 32], admitted: bool) -> Result<()> {
    ensure!(
        membership.generations().iter().any(|g| g.id == generation)
            && (generation == membership.current_generation().id || admitted),
        "error encrypted-tail-invalid"
    );
    Ok(())
}

pub mod envelope;
pub mod parent;

#[cfg(test)]
mod tests;
