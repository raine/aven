//! Structural staging views and component-bound retained-byte verification.
//! These checks do not establish current membership, expiry or server acceptance.

use super::catalog::{Artifact, Images};
use super::{Descriptor, Error, Result, catalog, decode_state_catalog};
use crate::context::LocalSharedStatePackageContext;
use crate::wire::bootstrap::Component;

pub struct DeclarationView(Descriptor);

pub struct Binding {
    pub vault: [u8; 32],
    pub generation: [u8; 32],
    pub bootstrap: [u8; 32],
    pub stream: [u8; 32],
    pub membership: [u8; 32],
}

impl DeclarationView {
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Descriptor::decode(bytes).map(Self)
    }

    pub fn prefix_count(&self) -> u64 {
        self.0.prefix
    }

    pub fn prefix_rows(&self, bytes: &[u8]) -> Result<Vec<(u64, String)>> {
        self.catalog(1, bytes)?;
        catalog::prefix_decode(bytes, self.0.prefix)
    }

    pub fn image_rows(&self, bytes: &[u8]) -> Result<Images> {
        self.catalog(2, bytes)?;
        Images::decode(bytes)
    }

    pub fn binding(&self) -> Binding {
        let d = &self.0;
        Binding {
            vault: d.vault,
            generation: d.generation,
            bootstrap: d.bootstrap,
            stream: d.stream,
            membership: d.membership,
        }
    }

    pub fn catalog_lengths(&self, class: usize) -> Result<Vec<u64>> {
        Ok(self
            .0
            .catalogs
            .get(class)
            .ok_or(Error::Invalid)?
            .slice_lengths())
    }

    pub fn verify_slice(&self, class: usize, index: usize, bytes: &[u8]) -> Result<()> {
        self.0
            .catalogs
            .get(class)
            .ok_or(Error::Invalid)?
            .verify_slice(index, bytes)
    }

    pub fn catalog(&self, class: usize, bytes: &[u8]) -> Result<CatalogView> {
        self.0
            .catalogs
            .get(class)
            .ok_or(Error::Invalid)?
            .verify(bytes, class as u8 + 1)?;
        let catalog = match class {
            0 => Ok(Catalog::State(decode_state_catalog(bytes)?)),
            1 => {
                catalog::prefix_decode(bytes, self.0.prefix)?;
                Ok(Catalog::Prefix)
            }
            2 => Ok(Catalog::Images(Images::decode(bytes)?)),
            _ => Err(Error::Invalid),
        }?;
        Ok(CatalogView(catalog))
    }

    pub fn artifacts(&self, catalog: Option<&CatalogView>) -> Vec<ArtifactView> {
        let d = &self.0;
        let wrap = |component, artifact: Artifact| ArtifactView {
            component,
            artifact,
            context: d.context(),
            stream: d.stream,
            bootstrap: d.bootstrap,
        };
        match catalog.map(|c| &c.0) {
            None => vec![wrap(Component::Manifest, d.manifest.clone())],
            Some(Catalog::State(a)) => vec![wrap(Component::State, a.clone())],
            Some(Catalog::Images(images)) => images
                .objects
                .iter()
                .map(|i| wrap(Component::Image(i.id), i.artifact.clone()))
                .collect(),
            Some(Catalog::Prefix) => Vec::new(),
        }
    }
}

pub struct CatalogView(Catalog);

enum Catalog {
    State(Artifact),
    Prefix,
    Images(Images),
}

pub struct ArtifactView {
    /// Structural routing selector, not proof of server admission.
    pub component: Component,
    artifact: Artifact,
    context: LocalSharedStatePackageContext,
    stream: [u8; 32],
    bootstrap: [u8; 32],
}

impl ArtifactView {
    pub fn lengths(&self) -> Vec<u64> {
        self.artifact.chunks.iter().map(|c| c.length).collect()
    }

    /// Exact descriptor recipe; callers must bind it to current stored inputs.
    pub fn artifact(&self) -> &Artifact {
        &self.artifact
    }

    pub fn context(&self) -> LocalSharedStatePackageContext {
        self.context
    }

    pub fn stream(&self) -> [u8; 32] {
        self.stream
    }

    /// Record identity, family and class used by the chunk-header verifier.
    pub fn identity(&self) -> Result<([u8; 32], u8, u8)> {
        match self.component {
            Component::Image(id) => Ok((id, 1, 0)),
            Component::State => Ok((self.bootstrap, 2, 1)),
            Component::Manifest => Ok((self.bootstrap, 2, 2)),
            _ => Err(Error::Invalid),
        }
    }

    pub fn verify_chunk(&self, index: usize, bytes: &[u8]) -> Result<()> {
        let (id, family, class) = self.identity()?;
        self.artifact
            .verify_chunk(bytes, index, self.context, self.stream, id, family, class)
    }

    pub fn verify(&self, records: &[Vec<u8>]) -> Result<()> {
        let (id, family, class) = self.identity()?;
        self.artifact
            .verify(records, self.context, self.stream, id, family, class)
    }
}

#[cfg(test)]
mod tests;
