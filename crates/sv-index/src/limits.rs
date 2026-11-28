//! Limits of `sv.plugin-index.v1` (contract M15 §1, §3).

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;

/// `index.json` size, counted on bytes read.
pub const MAX_INDEX_BYTES: u64 = 8 * MIB;
/// Any single file in the index repository.
pub const MAX_REPO_FILE_BYTES: u64 = MIB;
/// Package file (`sv-package` `Limits::max_zip_bytes`).
pub const MAX_PACKAGE_BYTES: u64 = 32 * MIB;
/// Icon file size.
pub const MAX_ICON_BYTES: u64 = 64 * KIB;
/// Icon edge, both sides equal, inclusive range.
pub const ICON_EDGE: std::ops::RangeInclusive<u32> = 64..=512;
/// Screenshots per plugin.
pub const MAX_SCREENSHOTS: usize = 8;
/// Screenshot file size.
pub const MAX_SCREENSHOT_BYTES: u64 = MIB;
/// Screenshot width and height, inclusive range.
pub const SCREENSHOT_EDGE: std::ops::RangeInclusive<u32> = 320..=2560;
/// Timestamps are Unix seconds in `[0, MAX_TIMESTAMP)`.
pub const MAX_TIMESTAMP: u64 = 100_000_000_000;

/// Versions per plugin.
pub const MAX_VERSIONS: usize = 200;
/// Keys per publisher.
pub const MAX_KEYS: usize = 8;
/// Maintainers per publisher.
pub const MAX_MAINTAINERS: usize = 10;
/// Languages per text map.
pub const MAX_LANGS: usize = 8;
/// Longest URL, in characters.
pub const MAX_URL_CHARS: usize = 512;
/// Longest image path, in characters.
pub const MAX_PATH_CHARS: usize = 200;

/// Name of a plugin, publisher, index or category (display title).
pub const MAX_TITLE_CHARS: usize = 80;
/// Index description.
pub const MAX_INDEX_DESCRIPTION_CHARS: usize = 500;
/// Plugin summary.
pub const MAX_SUMMARY_CHARS: usize = 140;
/// Plugin description (`CommonMark`).
pub const MAX_DESCRIPTION_CHARS: usize = 8000;
/// Screenshot caption.
pub const MAX_CAPTION_CHARS: usize = 140;
/// Version changelog.
pub const MAX_CHANGELOG_CHARS: usize = 4000;
/// Version `status_reason` and key `reason`.
pub const MAX_REASON_CHARS: usize = 500;
/// Keywords per plugin.
pub const MAX_KEYWORDS: usize = 16;
/// Characters per keyword.
pub const MAX_KEYWORD_CHARS: usize = 32;
/// Labels per plugin.
pub const MAX_LABELS: usize = 16;
/// License expression.
pub const MAX_LICENSE_CHARS: usize = 128;
/// Entries per `contents` list.
pub const MAX_CONTENT_ITEMS: usize = 256;
/// Characters per `contents` string.
pub const MAX_CONTENT_CHARS: usize = 128;

/// Findings kept per report.
pub const MAX_ISSUES: usize = 200;
