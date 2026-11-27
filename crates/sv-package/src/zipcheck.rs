//! Structural checks of the zip container that the `zip` crate does not make: the central
//! directory and the local headers must agree, and the file must consist of exactly the local
//! entries in order, the central directory and the end record, with nothing in between or after.
//!
//! The checks refuse (as `zip_invalid`): multi-disk archives, zip64 records and extra fields,
//! an end-of-central-directory comment, data descriptors and other general-purpose flags than
//! UTF-8 names, a local header that differs from its central entry in name, method, sizes or CRC,
//! overlapping or gapped entries, data before the first entry, and trailing data. Packages are
//! produced by `sv-pack`, which writes none of these, so nothing legitimate is lost.

use crate::error::{ErrorCode, PackageError, Result};

const EOCD_SIG: u32 = 0x0605_4b50;
const EOCD_LEN: usize = 22;
const ZIP64_LOCATOR_SIG: u32 = 0x0706_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const CENTRAL_LEN: usize = 46;
const LOCAL_SIG: u32 = 0x0403_4b50;
const LOCAL_LEN: usize = 30;
const FLAG_UTF8: u16 = 0x0800;
const ZIP64_EXTRA_ID: u16 = 0x0001;

fn bad(msg: impl Into<String>) -> PackageError {
    PackageError::new(ErrorCode::ZipInvalid, msg)
}

fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    let raw = b.get(at..at.saturating_add(2)).ok_or_else(|| bad("truncated record"))?;
    Ok(u16::from_le_bytes([raw[0], raw[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    let raw = b.get(at..at.saturating_add(4)).ok_or_else(|| bad("truncated record"))?;
    Ok(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))
}

fn to_usize(v: u32) -> Result<usize> {
    usize::try_from(v).map_err(|_| bad("offset out of range"))
}

/// What a central entry claims; the local header must say the same.
struct Entry {
    flags: u16,
    method: u16,
    crc: u32,
    csize: u32,
    usize_: u32,
    local_offset: usize,
    name_start: usize,
    name_len: usize,
}

fn has_zip64_extra(mut extra: &[u8]) -> Result<bool> {
    while !extra.is_empty() {
        let id = u16_at(extra, 0)?;
        let len = usize::from(u16_at(extra, 2)?);
        if id == ZIP64_EXTRA_ID {
            return Ok(true);
        }
        extra = extra.get(4 + len..).ok_or_else(|| bad("malformed extra field"))?;
    }
    Ok(false)
}

/// Checks the container structure of `bytes`.
///
/// # Errors
/// `zip_invalid` for any inconsistency listed in the module documentation.
pub fn check(bytes: &[u8]) -> Result<()> {
    let eocd = bytes
        .len()
        .checked_sub(EOCD_LEN)
        .ok_or_else(|| bad("too short for a zip file"))?;
    if u32_at(bytes, eocd)? != EOCD_SIG {
        return Err(bad(
            "end of central directory is not at the end of the file (comment or trailing data)",
        ));
    }
    if u16_at(bytes, eocd + 20)? != 0 {
        return Err(bad("end-of-central-directory comment refused"));
    }
    if eocd >= 20 && u32_at(bytes, eocd - 20)? == ZIP64_LOCATOR_SIG {
        return Err(bad("zip64 refused"));
    }
    let (disk, cd_disk, here, total) = (
        u16_at(bytes, eocd + 4)?,
        u16_at(bytes, eocd + 6)?,
        u16_at(bytes, eocd + 8)?,
        u16_at(bytes, eocd + 10)?,
    );
    if disk != 0 || cd_disk != 0 || here != total {
        return Err(bad("multi-disk archive refused"));
    }
    let cd_size = u32_at(bytes, eocd + 12)?;
    let cd_offset = u32_at(bytes, eocd + 16)?;
    if total == u16::MAX || cd_size == u32::MAX || cd_offset == u32::MAX {
        return Err(bad("zip64 refused"));
    }
    let (cd_size, cd_offset) = (to_usize(cd_size)?, to_usize(cd_offset)?);
    if cd_offset.checked_add(cd_size) != Some(eocd) {
        return Err(bad("central directory does not end where the end record starts"));
    }

    let mut entries = Vec::with_capacity(usize::from(total));
    let mut at = cd_offset;
    for _ in 0..total {
        if u32_at(bytes, at)? != CENTRAL_SIG {
            return Err(bad("bad central directory record"));
        }
        let (name_len, extra_len, comment_len) = (
            usize::from(u16_at(bytes, at + 28)?),
            usize::from(u16_at(bytes, at + 30)?),
            usize::from(u16_at(bytes, at + 32)?),
        );
        let entry = Entry {
            flags: u16_at(bytes, at + 8)?,
            method: u16_at(bytes, at + 10)?,
            crc: u32_at(bytes, at + 16)?,
            csize: u32_at(bytes, at + 20)?,
            usize_: u32_at(bytes, at + 24)?,
            local_offset: to_usize(u32_at(bytes, at + 42)?)?,
            name_start: at + CENTRAL_LEN,
            name_len,
        };
        if u16_at(bytes, at + 34)? != 0 {
            return Err(bad("entry starts on another disk"));
        }
        if entry.csize == u32::MAX || entry.usize_ == u32::MAX || u32_at(bytes, at + 42)? == u32::MAX {
            return Err(bad("zip64 refused"));
        }
        if entry.flags & !FLAG_UTF8 != 0 {
            return Err(bad("data descriptors, encryption and other flags refused"));
        }
        let extra_start = entry.name_start + name_len;
        let extra = bytes
            .get(extra_start..extra_start + extra_len)
            .ok_or_else(|| bad("truncated central directory record"))?;
        if has_zip64_extra(extra)? {
            return Err(bad("zip64 refused"));
        }
        at = extra_start + extra_len + comment_len;
        entries.push(entry);
    }
    if at != eocd {
        return Err(bad("central directory has unaccounted bytes"));
    }

    let mut spans: Vec<(usize, usize)> = Vec::with_capacity(entries.len());
    for e in &entries {
        spans.push(check_local(bytes, e, cd_offset)?);
    }
    spans.sort_unstable();
    let mut expected = 0_usize;
    for (start, end) in spans {
        if start != expected {
            return Err(bad(if start < expected {
                "entries overlap"
            } else {
                "data before, between or outside the entries"
            }));
        }
        expected = end;
    }
    if expected != cd_offset {
        return Err(bad("data between the last entry and the central directory"));
    }
    Ok(())
}

/// Compares one local header with its central entry; returns the span `[start, end)` of the
/// header plus data.
fn check_local(bytes: &[u8], e: &Entry, cd_offset: usize) -> Result<(usize, usize)> {
    let at = e.local_offset;
    if u32_at(bytes, at)? != LOCAL_SIG {
        return Err(bad("bad local header"));
    }
    let name_len = usize::from(u16_at(bytes, at + 26)?);
    let extra_len = usize::from(u16_at(bytes, at + 28)?);
    let same = u16_at(bytes, at + 6)? == e.flags
        && u16_at(bytes, at + 8)? == e.method
        && u32_at(bytes, at + 14)? == e.crc
        && u32_at(bytes, at + 18)? == e.csize
        && u32_at(bytes, at + 22)? == e.usize_
        && name_len == e.name_len;
    if !same {
        return Err(bad("local header differs from the central directory"));
    }
    let local_name = bytes
        .get(at + LOCAL_LEN..at + LOCAL_LEN + name_len)
        .ok_or_else(|| bad("truncated local header"))?;
    let central_name = bytes
        .get(e.name_start..e.name_start + e.name_len)
        .ok_or_else(|| bad("truncated central directory record"))?;
    if local_name != central_name {
        return Err(bad("local header names a different file"));
    }
    if e.method == 0 && e.csize != e.usize_ {
        return Err(bad("stored entry with different sizes"));
    }
    let data_start = at + LOCAL_LEN + name_len + extra_len;
    let end = data_start
        .checked_add(to_usize(e.csize)?)
        .ok_or_else(|| bad("entry size out of range"))?;
    if end > cd_offset {
        return Err(bad("entry runs into the central directory"));
    }
    Ok((at, end))
}
