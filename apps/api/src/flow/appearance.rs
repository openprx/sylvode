#![allow(clippy::items_after_statements)]
//! Page icon and cover stored on `flow_objects.appearance`.
//!
//! The collaborative document is not the source of this metadata: the navigator and breadcrumbs
//! read it without opening the CRDT. Uploaded bytes live in `flow_appearance_assets` and are
//! served only to a caller who can view the object, or through a public share that currently
//! references the asset.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::error::ApiError;

const ASSET_BYTES_MAX: usize = 2_097_152;
const EMOJI_CHARS_MAX: usize = 16;
pub const GALLERY_IDS: &[&str] = &["dawn", "ocean", "moss", "dusk", "sand", "ink"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PageAppearance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<PageIcon>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<PageCover>,
}

impl PageAppearance {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.icon.is_none() && self.cover.is_none()
    }

    #[must_use]
    pub fn references_asset(&self, asset_id: Uuid) -> bool {
        let icon = matches!(self.icon, Some(PageIcon::Image { asset_id: id }) if id == asset_id);
        let cover = matches!(self.cover, Some(PageCover::Upload { asset_id: id, .. }) if id == asset_id);
        icon || cover
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageIcon {
    Emoji { emoji: String },
    Image { asset_id: Uuid },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageCover {
    Gallery { gallery_id: String, position: f64 },
    Upload { asset_id: Uuid, position: f64 },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppearanceAssetCreated {
    pub asset_id: Uuid,
    pub content_type: String,
    pub byte_size: usize,
}

#[derive(Debug)]
pub struct AppearanceAssetBytes {
    pub content_type: &'static str,
    pub bytes: Vec<u8>,
}

/// Reads stored JSON. A malformed value is empty appearance, not a failed object read.
#[must_use]
pub fn parse_stored(value: &Value) -> PageAppearance {
    match serde_json::from_value::<PageAppearance>(value.clone()) {
        Ok(appearance) => appearance,
        Err(error) => {
            tracing::warn!(error = %error, "flow appearance ignored because it is not valid");
            PageAppearance::default()
        }
    }
}

pub fn validate_appearance(appearance: &PageAppearance) -> Result<(), ApiError> {
    if let Some(icon) = &appearance.icon {
        match icon {
            PageIcon::Emoji { emoji } => validate_emoji(emoji)?,
            PageIcon::Image { .. } => {}
        }
    }
    if let Some(cover) = &appearance.cover {
        match cover {
            PageCover::Gallery { gallery_id, position } => {
                if !GALLERY_IDS.contains(&gallery_id.as_str()) {
                    return Err(ApiError::invalid_update("unknown cover gallery id".to_string()));
                }
                validate_position(*position)?;
            }
            PageCover::Upload { position, .. } => validate_position(*position)?,
        }
    }
    let encoded = serde_json::to_vec(appearance).map_err(|_| ApiError::internal())?;
    if encoded.len() > 4_096 {
        return Err(ApiError::invalid_update("appearance metadata is too large".to_string()));
    }
    Ok(())
}

pub fn decode_image(data_base64: &str) -> Result<(&'static str, Vec<u8>), ApiError> {
    let trimmed = data_base64.trim();
    if trimmed.len() > ASSET_BYTES_MAX.saturating_mul(2) {
        return Err(ApiError::limit_exceeded(
            "appearance image exceeds the size limit",
            "appearance_asset_bytes",
            Some(serde_json::json!(ASSET_BYTES_MAX)),
            Some(serde_json::json!(trimmed.len())),
            None,
        ));
    }
    let bytes = STANDARD
        .decode(trimmed)
        .map_err(|_| ApiError::invalid_update("appearance image is not valid base64".to_string()))?;
    if bytes.is_empty() || bytes.len() > ASSET_BYTES_MAX {
        return Err(ApiError::limit_exceeded(
            "appearance image exceeds the size limit",
            "appearance_asset_bytes",
            Some(serde_json::json!(ASSET_BYTES_MAX)),
            Some(serde_json::json!(bytes.len())),
            None,
        ));
    }
    let content_type = sniff_image(&bytes)
        .ok_or_else(|| ApiError::invalid_update("appearance image must be png, jpeg, webp, or gif".to_string()))?;
    Ok((content_type, bytes))
}

pub(crate) fn referenced_assets(appearance: &PageAppearance) -> Vec<Uuid> {
    let mut ids = Vec::new();
    if let Some(PageIcon::Image { asset_id }) = appearance.icon {
        ids.push(asset_id);
    }
    if let Some(PageCover::Upload { asset_id, .. }) = appearance.cover {
        ids.push(asset_id);
    }
    ids
}

fn validate_emoji(emoji: &str) -> Result<(), ApiError> {
    let trimmed = emoji.trim();
    let count = trimmed.chars().count();
    if count == 0 || count > EMOJI_CHARS_MAX {
        return Err(ApiError::invalid_update(
            "page icon emoji must be 1-16 characters".to_string(),
        ));
    }
    let mut symbolic = false;
    for ch in trimmed.chars() {
        if ch.is_control() || ch.is_ascii_alphanumeric() || matches!(ch, '<' | '>' | '&' | '"' | '\'') {
            return Err(ApiError::invalid_update("page icon emoji is not valid".to_string()));
        }
        if !ch.is_ascii() {
            symbolic = true;
        }
    }
    if !symbolic {
        return Err(ApiError::invalid_update("page icon emoji is not valid".to_string()));
    }
    Ok(())
}

fn validate_position(position: f64) -> Result<(), ApiError> {
    if position.is_finite() && (0.0..=1.0).contains(&position) {
        Ok(())
    } else {
        Err(ApiError::invalid_update(
            "cover position must be between 0 and 1".to_string(),
        ))
    }
}

#[must_use]
pub fn sniff_image(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n']) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.get(..4) == Some(b"RIFF".as_slice()) && bytes.get(8..12) == Some(b"WEBP".as_slice()) {
        return Some("image/webp");
    }
    None
}

pub(crate) fn static_content_type(value: &str) -> Option<&'static str> {
    match value {
        "image/png" => Some("image/png"),
        "image/jpeg" => Some("image/jpeg"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{GALLERY_IDS, decode_image, sniff_image, validate_appearance, validate_emoji};
    use super::{PageAppearance, PageCover, PageIcon};
    use uuid::Uuid;

    #[test]
    fn emoji_accepts_a_symbol_and_rejects_plain_text() {
        assert!(validate_emoji("📄").is_ok());
        assert!(validate_emoji("abc").is_err());
        assert!(validate_emoji("<img>").is_err());
    }

    #[test]
    fn gallery_cover_must_be_a_known_id_and_a_finite_position() {
        let ok = PageAppearance {
            icon: Some(PageIcon::Emoji {
                emoji: "🌿".to_string(),
            }),
            cover: Some(PageCover::Gallery {
                gallery_id: GALLERY_IDS.first().copied().unwrap_or("dawn").to_string(),
                position: 0.25,
            }),
        };
        assert!(validate_appearance(&ok).is_ok());
        let bad = PageAppearance {
            icon: None,
            cover: Some(PageCover::Gallery {
                gallery_id: "nope".to_string(),
                position: 0.5,
            }),
        };
        assert!(validate_appearance(&bad).is_err());
        let nan = PageAppearance {
            icon: None,
            cover: Some(PageCover::Upload {
                asset_id: Uuid::nil(),
                position: f64::NAN,
            }),
        };
        assert!(validate_appearance(&nan).is_err());
    }

    #[test]
    fn image_bytes_must_match_a_known_signature() {
        assert_eq!(sniff_image(b"GIF89a"), Some("image/gif"));
        assert!(sniff_image(b"<svg></svg>").is_none());
        assert!(decode_image("not base64!!!").is_err());
    }

    #[test]
    fn appearance_tracks_which_uploaded_asset_it_shows() {
        let asset_id = Uuid::from_u128(7);
        let appearance = PageAppearance {
            icon: Some(PageIcon::Image { asset_id }),
            cover: None,
        };
        assert!(appearance.references_asset(asset_id));
        assert!(!appearance.references_asset(Uuid::from_u128(8)));
    }
}
