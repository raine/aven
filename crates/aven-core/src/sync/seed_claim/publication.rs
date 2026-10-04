//! Package-authenticated publication signing for local snapshots.
use super::*;
use crate::sync::bootstrap_format;
pub use aven_protocol::claim::publication::{
    PUBLICATION_BYTES, Publication, PublicationBinding, PublicationOutcome,
};
impl SeedAuthority {
    /// Authenticates the frozen package and signs bounded publication intent.
    pub fn prepare_bootstrap_publication(
        &self,
        package: &bootstrap_format::Package,
        key: &LocalSharedStatePackageKey,
    ) -> Result<Publication> {
        self.validate(self.genesis().context(), key)?;
        let binding = PublicationBinding::from_descriptor(self.genesis(), &package.descriptor)?;
        bootstrap_format::authenticate(
            package,
            key,
            self.genesis().context(),
            binding.stream_id,
            binding.bootstrap_id,
            self.genesis().commitment(),
        )?;
        self.sign_authenticated_publication(&package.descriptor, key)
    }

    pub(crate) fn sign_authenticated_publication(
        &self,
        descriptor: &[u8],
        key: &LocalSharedStatePackageKey,
    ) -> Result<Publication> {
        self.0.sign_authenticated_publication(descriptor, key)
    }
}
