//! Typed package errors.

use std::fmt;

/// Machine-readable error code; the names are the `details.code` values of `package_invalid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    /// A size limit is exceeded (zip, entry, total or compression ratio).
    TooLarge,
    /// More than 512 entries.
    TooManyEntries,
    /// An entry path is not acceptable (absolute, `..`, bad characters, duplicate).
    PathInvalid,
    /// Symlink, directory, encrypted or otherwise non-regular entry, or an unsupported method.
    EntryType,
    /// A zip entry is not listed in the manifest `files` map.
    FileUnlisted,
    /// A file listed in the manifest is not in the zip.
    FileMissing,
    /// A content file does not match its digest in the manifest.
    DigestMismatch,
    /// The signature does not verify, or is malformed.
    SignatureInvalid,
    /// No key is registered for the manifest's `key_id`.
    KeyUnknown,
    /// The key is revoked.
    KeyRevoked,
    /// The manifest is malformed or inconsistent.
    ManifestInvalid,
    /// The zip is corrupt or unreadable, or its local headers, central directory, offsets or
    /// trailing data are inconsistent.
    ZipInvalid,
    /// Key material is unusable. Not in C3's list.
    KeyInvalid,
}

impl ErrorCode {
    /// The `snake_case` name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TooLarge => "too_large",
            Self::TooManyEntries => "too_many_entries",
            Self::PathInvalid => "path_invalid",
            Self::EntryType => "entry_type",
            Self::FileUnlisted => "file_unlisted",
            Self::FileMissing => "file_missing",
            Self::DigestMismatch => "digest_mismatch",
            Self::SignatureInvalid => "signature_invalid",
            Self::KeyUnknown => "key_unknown",
            Self::KeyRevoked => "key_revoked",
            Self::ManifestInvalid => "manifest_invalid",
            Self::ZipInvalid => "zip_invalid",
            Self::KeyInvalid => "key_invalid",
        }
    }
}

/// A package error: a code and an English message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageError {
    /// The code.
    pub code: ErrorCode,
    /// Detail for logs and the upload response.
    pub message: String,
}

impl PackageError {
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for PackageError {}

/// Result alias.
pub type Result<T> = std::result::Result<T, PackageError>;
