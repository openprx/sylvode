//! Image rules: format by magic bytes (PNG, JPEG, WebP; SVG refused), dimensions from the header,
//! size and dimension limits per role. No pixel data is decoded.

use std::fmt;

use crate::limits;
pub use crate::rules::ImageFormat;

/// What an image is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageRole {
    /// Plugin icon: square, 64..=512 px, at most 64 KiB.
    Icon,
    /// Screenshot: 320..=2560 px each side, at most 1 MiB.
    Screenshot,
}

/// What the header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageInfo {
    /// Format by magic bytes.
    pub format: ImageFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

/// An image that breaks a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageError(pub String);

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ImageError {}

fn err<T>(msg: &str) -> Result<T, ImageError> {
    Err(ImageError(msg.to_owned()))
}

fn be16(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u32::from(u16::from_be_bytes([*s.first()?, *s.get(1)?])))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_be_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

fn le16(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u32::from(u16::from_le_bytes([*s.first()?, *s.get(1)?])))
}

fn le24(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(3)?)?;
    Some(u32::from(*s.first()?) | (u32::from(*s.get(1)?) << 8) | (u32::from(*s.get(2)?) << 16))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([*s.first()?, *s.get(1)?, *s.get(2)?, *s.get(3)?]))
}

fn png(b: &[u8]) -> Option<ImageInfo> {
    // Signature, then the IHDR chunk: length 13, type, width, height.
    if be32(b, 8)? != 13 || b.get(12..16)? != b"IHDR" {
        return None;
    }
    Some(ImageInfo {
        format: ImageFormat::Png,
        width: be32(b, 16)?,
        height: be32(b, 20)?,
    })
}

fn jpeg(b: &[u8]) -> Option<ImageInfo> {
    let mut i = 2usize;
    loop {
        if *b.get(i)? != 0xFF {
            return None;
        }
        let marker = *b.get(i.checked_add(1)?)?;
        match marker {
            0xFF => {
                i = i.checked_add(1)?;
                continue;
            }
            // Standalone markers carry no length.
            0x01 | 0xD0..=0xD8 => {
                i = i.checked_add(2)?;
                continue;
            }
            // End of image or start of scan before any frame header.
            0xD9 | 0xDA => return None,
            _ => {}
        }
        let len = usize::try_from(be16(b, i.checked_add(2)?)?).ok()?;
        if len < 2 {
            return None;
        }
        if matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            return Some(ImageInfo {
                format: ImageFormat::Jpeg,
                height: be16(b, i.checked_add(5)?)?,
                width: be16(b, i.checked_add(7)?)?,
            });
        }
        i = i.checked_add(2)?.checked_add(len)?;
    }
}

fn webp(b: &[u8]) -> Option<ImageInfo> {
    let (width, height) = match b.get(12..16)? {
        b"VP8 " => {
            if b.get(23..26)? != [0x9D, 0x01, 0x2A] {
                return None;
            }
            (le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF)
        }
        b"VP8L" => {
            if *b.get(20)? != 0x2F {
                return None;
            }
            let bits = le32(b, 21)?;
            ((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1)
        }
        b"VP8X" => (le24(b, 24)? + 1, le24(b, 27)? + 1),
        _ => return None,
    };
    Some(ImageInfo {
        format: ImageFormat::Webp,
        width,
        height,
    })
}

fn looks_like_svg(b: &[u8]) -> bool {
    let head = b.get(..b.len().min(512)).unwrap_or_default();
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start().to_ascii_lowercase();
    t.starts_with("<svg") || t.starts_with("<?xml") || t.starts_with("<!doctype svg")
}

/// Identifies the format by magic bytes and reads the dimensions from the header.
pub fn sniff(bytes: &[u8]) -> Result<ImageInfo, ImageError> {
    let found = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        jpeg(bytes)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        webp(bytes)
    } else if looks_like_svg(bytes) {
        return err("SVG is not allowed");
    } else {
        return err("not a PNG, JPEG or WebP image");
    };
    match found {
        Some(info) if info.width > 0 && info.height > 0 => Ok(info),
        _ => err("image header is truncated or malformed"),
    }
}

/// Checks an image file for its role: size, magic bytes, dimensions, and (when `path` is given) that
/// the file extension names the sniffed format.
pub fn check_image(role: ImageRole, path: Option<&str>, bytes: &[u8]) -> Result<ImageInfo, ImageError> {
    let (max_bytes, what) = match role {
        ImageRole::Icon => (limits::MAX_ICON_BYTES, "icon"),
        ImageRole::Screenshot => (limits::MAX_SCREENSHOT_BYTES, "screenshot"),
    };
    if bytes.len() as u64 > max_bytes {
        return Err(ImageError(format!(
            "{what} is {} bytes, at most {max_bytes} allowed",
            bytes.len()
        )));
    }
    let info = sniff(bytes)?;
    if let Some(ext) = path.and_then(|p| p.rsplit_once('.')).map(|(_, e)| e)
        && crate::rules::ImageFormat::from_extension(ext) != Some(info.format)
    {
        return err("file extension does not match the image format");
    }
    match role {
        ImageRole::Icon => {
            if info.width != info.height || !limits::ICON_EDGE.contains(&info.width) {
                return Err(ImageError(format!(
                    "icon is {}x{}, must be square, 64..512 px",
                    info.width, info.height
                )));
            }
        }
        ImageRole::Screenshot => {
            if !limits::SCREENSHOT_EDGE.contains(&info.width) || !limits::SCREENSHOT_EDGE.contains(&info.height) {
                return Err(ImageError(format!(
                    "screenshot is {}x{}, each side must be 320..2560 px",
                    info.width, info.height
                )));
            }
        }
    }
    Ok(info)
}
