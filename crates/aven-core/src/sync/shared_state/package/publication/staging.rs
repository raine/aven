//! Transaction-local structural views of the actual publication codec.

use super::*;
use crate::sync::bootstrap_staging::Component;

pub(crate) struct DeclarationView(Descriptor);

pub(crate) struct Binding {
    pub(crate) vault: [u8; 32],
    pub(crate) generation: [u8; 32],
    pub(crate) bootstrap: [u8; 32],
    pub(crate) stream: [u8; 32],
    pub(crate) membership: [u8; 32],
}

impl DeclarationView {
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self> {
        Descriptor::decode(bytes).map(Self)
    }

    pub(crate) fn prefix_count(&self) -> u64 {
        self.0.prefix
    }

    pub(crate) fn manifest_commitment(&self) -> [u8; 32] {
        self.0.manifest.aggregate
    }

    pub(crate) fn prefix_rows(&self, bytes: &[u8]) -> Result<Vec<(u64, String)>> {
        self.catalog(1, bytes)?;
        catalog::prefix_decode(bytes, self.0.prefix)
    }

    pub(crate) fn image_rows(&self, bytes: &[u8]) -> Result<Images> {
        self.catalog(2, bytes)?;
        Images::decode(bytes)
    }

    pub(crate) fn binding(&self) -> Binding {
        let d = &self.0;
        Binding {
            vault: d.vault,
            generation: d.generation,
            bootstrap: d.bootstrap,
            stream: d.stream,
            membership: d.membership,
        }
    }

    pub(crate) fn catalog_lengths(&self, class: usize) -> Result<Vec<u64>> {
        let d = self.0.catalogs.get(class).ok_or(Error::Invalid)?;
        Ok((0..count(d.length))
            .map(|i| (d.length - i * CHUNK).min(CHUNK))
            .collect())
    }

    pub(crate) fn catalog(&self, class: usize, bytes: &[u8]) -> Result<CatalogView> {
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

    pub(crate) fn artifacts(&self, catalog: Option<&CatalogView>) -> Vec<ArtifactView> {
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

pub(crate) struct CatalogView(Catalog);

enum Catalog {
    State(Artifact),
    Prefix,
    Images(Images),
}

pub(crate) struct ArtifactView {
    pub(crate) component: Component,
    artifact: Artifact,
    context: crypto::LocalSharedStatePackageContext,
    stream: [u8; 32],
    bootstrap: [u8; 32],
}

impl ArtifactView {
    pub(crate) fn lengths(&self) -> Vec<u64> {
        self.artifact.chunks.iter().map(|c| c.length).collect()
    }

    fn identity(&self) -> ([u8; 32], u8, u8) {
        match self.component {
            Component::Image(id) => (id, 1, 0),
            Component::State => (self.bootstrap, 2, 1),
            Component::Manifest => (self.bootstrap, 2, 2),
            _ => unreachable!("catalogs are not encrypted artifacts"),
        }
    }

    pub(crate) fn verify_chunk(&self, index: usize, bytes: &[u8]) -> Result<()> {
        let (id, family, class) = self.identity();
        self.artifact
            .verify_chunk(bytes, index, self.context, self.stream, id, family, class)
    }

    pub(crate) fn verify(&self, records: &[Vec<u8>]) -> Result<()> {
        let (id, family, class) = self.identity();
        self.artifact
            .verify(records, self.context, self.stream, id, family, class)
    }
}
