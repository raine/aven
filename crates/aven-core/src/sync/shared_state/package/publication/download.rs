//! Download recipes use the publication codec, not a second snapshot format.
use super::*;
use crate::sync::bootstrap_staging::Component;

/// Expected ordered lengths, derived only from a verified complete catalog.
pub struct Recipe {
    pub component: Component,
    pub lengths: Vec<u64>,
}

pub fn catalogs(descriptor: &[u8]) -> Result<Vec<Recipe>> {
    let d = staging::DeclarationView::decode(descriptor)?;
    [
        Component::DataCatalog,
        Component::PrefixCatalog,
        Component::ImageCatalog,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, component)| {
        Ok(Recipe {
            component,
            lengths: d.catalog_lengths(i)?,
        })
    })
    .collect()
}

pub fn artifacts(descriptor: &[u8], catalogs: &[Vec<u8>; 3]) -> Result<Vec<Recipe>> {
    let d = staging::DeclarationView::decode(descriptor)?;
    let mut artifacts = d.artifacts(None);
    for (i, bytes) in catalogs.iter().enumerate() {
        artifacts.extend(d.artifacts(Some(&d.catalog(i, bytes)?)));
    }
    Ok(artifacts
        .into_iter()
        .map(|a| Recipe {
            component: a.component,
            lengths: a.lengths(),
        })
        .collect())
}

/// Published metadata required for a fresh join, independent of image storage.
pub struct Metadata {
    pub descriptor: Vec<u8>,
    pub catalogs: [Vec<u8>; 3],
    pub state: Vec<Vec<u8>>,
    pub manifest: Vec<Vec<u8>>,
}

pub(super) struct MetadataView<'a> {
    pub descriptor: &'a [u8],
    pub catalogs: &'a [Vec<u8>; 3],
    pub state: &'a [Vec<u8>],
    pub manifest: &'a [Vec<u8>],
}
impl<'a> From<&'a Package> for MetadataView<'a> {
    fn from(p: &'a Package) -> Self {
        Self {
            descriptor: &p.descriptor,
            catalogs: &p.catalogs,
            state: &p.state,
            manifest: &p.manifest,
        }
    }
}

pub(crate) struct VerifiedMetadata {
    pub capture: SharedStateCapture,
    pub index: AttachmentIndex,
}

pub(crate) fn decrypt(
    metadata: &Metadata,
    key: &LocalSharedStatePackageKey,
) -> anyhow::Result<VerifiedMetadata> {
    let view = MetadataView {
        descriptor: &metadata.descriptor,
        catalogs: &metadata.catalogs,
        state: &metadata.state,
        manifest: &metadata.manifest,
    };
    let (capture, mappings) = decrypt_metadata(&view, key)?;
    let index = index_from_mappings(&view, &mappings)?;
    Ok(VerifiedMetadata { capture, index })
}
