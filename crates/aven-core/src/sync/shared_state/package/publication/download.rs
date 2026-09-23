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

pub(crate) struct VerifiedContent {
    pub capture: SharedStateCapture,
    pub images: Vec<(String, Zeroizing<Vec<u8>>)>,
}

pub(crate) fn decrypt(
    package: &Package,
    key: &LocalSharedStatePackageKey,
) -> Result<VerifiedContent> {
    let mut images = Vec::new();
    let (capture, _) = decrypt_domain_images(package, key, |hash, bytes| {
        images.push((hash.to_owned(), bytes))
    })?;
    Ok(VerifiedContent { capture, images })
}
