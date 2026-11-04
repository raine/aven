//! Images a person attaches: which files count, how they become storable
//! bytes, and the thumbnails that preview them.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, ClipboardEntry};

use crate::adapter::PendingImage;

const IMAGE_EXTENSIONS: [&str; 8] = ["png", "jpg", "jpeg", "gif", "webp", "heic", "tiff", "bmp"];

/// An image from the clipboard: its bytes, or the file it was copied from.
#[derive(Clone)]
pub enum PastedImage {
    Bytes(gpui::Image),
    Path(PathBuf),
}

/// An image the clipboard holds, if any. Other copied files read as nothing,
/// so their names still paste as text.
pub fn clipboard_image(cx: &App) -> Option<PastedImage> {
    let item = cx.read_from_clipboard()?;
    item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(PastedImage::Bytes(image.clone())),
        ClipboardEntry::ExternalPaths(paths) => paths
            .paths()
            .iter()
            .find(|path| is_image(path))
            .cloned()
            .map(PastedImage::Path),
        ClipboardEntry::String(_) => None,
    })
}

pub fn is_image(path: &Path) -> bool {
    let extension = path.extension().and_then(|extension| extension.to_str());
    extension.is_some_and(|extension| IMAGE_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// Reads an image from disk, by the name it will keep. Other files read as nothing.
pub fn read_image(path: &Path) -> Option<PendingImage> {
    if !is_image(path) {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let name = path.file_name()?.to_string_lossy().to_string();
    Some(storable(name, bytes))
}

/// The image a paste carries.
pub fn pasted_image(pasted: PastedImage) -> Option<PendingImage> {
    match pasted {
        PastedImage::Path(path) => read_image(&path),
        PastedImage::Bytes(image) => Some(storable("pasted.png".to_string(), image.bytes)),
    }
}

/// Aven stores PNG, JPEG, GIF and WebP. Other formats that decode here, such
/// as the TIFF many macOS apps copy, become PNG; the rest go through as they
/// are, so the store can say what's wrong with them.
fn storable(name: String, bytes: Vec<u8>) -> PendingImage {
    use image::ImageFormat;
    let stored = matches!(
        image::guess_format(&bytes),
        Ok(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)
    );
    match (!stored).then(|| as_png(&bytes)).flatten() {
        Some(png) => PendingImage {
            name: Path::new(&name)
                .with_extension("png")
                .to_string_lossy()
                .to_string(),
            bytes: Arc::new(png),
        },
        None => PendingImage {
            name,
            bytes: Arc::new(bytes),
        },
    }
}

fn as_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let picture = image::load_from_memory(bytes).ok()?;
    let mut png = Vec::new();
    picture
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// A picture of an image cropped to fill `width` by `height` pixels, or none
/// when the bytes do not decode.
pub fn thumbnail(bytes: &[u8], width: u32, height: u32) -> Option<gpui::RenderImage> {
    let picture = image::load_from_memory(bytes).ok()?;
    let mut pixels = picture
        .resize_to_fill(width, height, image::imageops::FilterType::Triangle)
        .into_rgba8();
    // GPUI paints BGRA.
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Some(gpui::RenderImage::new(vec![image::Frame::new(pixels)]))
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    #[test]
    fn a_thumbnail_fills_its_size_and_junk_has_none() {
        let thumb = thumbnail(&encoded(image::ImageFormat::Png), 6, 4).unwrap();
        assert_eq!(thumb.size(0), size(6.into(), 4.into()));
        assert!(thumbnail(b"not an image", 6, 4).is_none());
    }

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbImage::new(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn every_format_storable_converts_counts_as_an_image() {
        for name in ["a.TIFF", "b.bmp", "c.png"] {
            assert!(is_image(Path::new(name)), "{name}");
        }
        assert!(!is_image(Path::new("notes.txt")));
    }

    #[test]
    fn a_pasted_tiff_becomes_a_png() {
        let image = storable("pasted.png".to_string(), encoded(image::ImageFormat::Tiff));
        assert_eq!(image.name, "pasted.png");
        assert_eq!(
            image::guess_format(&image.bytes).unwrap(),
            image::ImageFormat::Png
        );
    }

    #[test]
    fn a_tiff_file_keeps_its_name_with_a_png_extension() {
        let image = storable("scan.tiff".to_string(), encoded(image::ImageFormat::Tiff));
        assert_eq!(image.name, "scan.png");
    }

    #[test]
    fn stored_formats_and_undecodable_bytes_pass_through() {
        let png = encoded(image::ImageFormat::Png);
        assert_eq!(*storable("a.png".to_string(), png.clone()).bytes, png);
        let junk = b"not an image".to_vec();
        let image = storable("photo.heic".to_string(), junk.clone());
        assert_eq!((image.name.as_str(), &*image.bytes), ("photo.heic", &junk));
    }
}
