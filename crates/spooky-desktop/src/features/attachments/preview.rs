//! A small picture of an attached image, for the composer to show.
//!
//! The file is the user's own choice, but it is still only data: it is decoded with bounds
//! and drawn, never run. A file that is not an image, is too large, or cannot be decoded
//! has no preview, and the attachment is still named by its path.

use image::{ImageFormat, ImageReader, imageops::FilterType};
use std::{io::Cursor, path::Path};

/// Largest file decoded for a preview.
const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Largest side of the preview, in pixels.
const MAX_SIDE: u32 = 320;

/// A preview of an attached image, ready for both interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Preview {
    /// The small picture as PNG bytes, for the native view.
    pub(crate) png: Vec<u8>,
    /// The same picture as a `data:` URI, for the web view.
    pub(crate) data_uri: String,
    /// The picture's own size, before shrinking.
    pub(crate) width: u32,
    pub(crate) height: u32,
}

/// The preview of `path`, or `None` when it is not a picture this can read.
pub(crate) fn preview(path: &Path) -> Option<Preview> {
    if path.metadata().ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let decoded = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    // Only formats this build can read; anything else is named, not drawn.
    let image = decoded.decode().ok()?;
    let (width, height) = (image.width(), image.height());
    let small = if width.max(height) > MAX_SIDE {
        image.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        image
    };
    let mut png = Vec::new();
    small
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    let data_uri = format!("data:image/png;base64,{}", base64(&png));
    Some(Preview {
        png,
        data_uri,
        width,
        height,
    })
}

/// Standard base64, as a `data:` URI carries it.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let (a, b, c) = (
            u32::from(chunk[0]),
            chunk.get(1).map_or(0, |b| u32::from(*b)),
            chunk.get(2).map_or(0, |c| u32::from(*c)),
        );
        let triple = (a << 16) | (b << 8) | c;
        let digit = |shift: u32| char::from(ALPHABET[((triple >> shift) & 0x3f) as usize]);
        out.push(digit(18));
        out.push(digit(12));
        out.push(if chunk.len() > 1 { digit(6) } else { '=' });
        out.push(if chunk.len() > 2 { digit(0) } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    use std::{env, fs, process};

    /// A directory removed when it goes out of scope.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("spooky-preview-{}-{name}", process::id()));
            fs::create_dir_all(&dir).expect("creating a scratch directory");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn picture(dir: &Path, name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let mut image = RgbaImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
        let path = dir.join(name);
        image.save(&path).expect("writing the fixture");
        path
    }

    #[test]
    fn base64_follows_the_standard_including_its_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xef, 0xbe]), "/+++");
    }

    #[test]
    fn a_big_picture_is_shrunk_and_keeps_its_own_size() {
        let dir = Scratch::new("big");
        let found = preview(&picture(&dir.0, "big.png", 1000, 500)).expect("a preview");
        assert_eq!((found.width, found.height), (1000, 500));
        let small = image::load_from_memory(&found.png).expect("the preview is a picture");
        assert_eq!((small.width(), small.height()), (MAX_SIDE, MAX_SIDE / 2));
        assert!(found.data_uri.starts_with("data:image/png;base64,iVBOR"));
    }

    #[test]
    fn a_small_picture_is_kept_as_it_is() {
        let dir = Scratch::new("small");
        let found = preview(&picture(&dir.0, "small.png", 40, 20)).expect("a preview");
        let small = image::load_from_memory(&found.png).expect("the preview is a picture");
        assert_eq!((small.width(), small.height()), (40, 20));
    }

    #[test]
    fn what_is_not_a_picture_has_no_preview() {
        let dir = Scratch::new("other");
        let text = dir.0.join("notes.txt");
        fs::write(&text, "hello").expect("writing the fixture");
        assert_eq!(preview(&text), None);
        assert_eq!(preview(&dir.0.join("missing.png")), None);
    }
}
