//! The opaque resume cursor of the event stream.
//!
//! A cursor names a position in the feed: the sequence number of the last event a client applied
//! and the time that event was written. It is what travels in the `id:` line of every event and
//! comes back in `Last-Event-ID` (or the `cursor` query parameter).
//!
//! The sequence is shared by every workspace, so a plain number would tell a client how many
//! events it is *not* allowed to see (the gaps between its own events). The cursor is therefore
//! sealed with AES-256-GCM:
//!
//! * the sequence and the time are encrypted, so a client learns nothing from the value and sees
//!   no gaps;
//! * the grant id is authenticated data, so a cursor taken from another grant, or invented, does
//!   not open and is refused instead of being trusted;
//! * the nonce is a keyed hash of the grant and the position, so the same position of the same
//!   grant always seals to the same text. That keeps the event id stable across reconnects,
//!   which is what a client de-duplicates on.
//!
//! The key is derived from the instance's signing secret, so every API process agrees on it and
//! rotating the secret invalidates outstanding cursors (a client then gets a 400 and resynchronises
//! by reading, like any other unusable cursor). No key or plaintext position is ever logged.

use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::hmac;
use uuid::Uuid;

const VERSION: u8 = 1;
const PLAIN_LEN: usize = 1 + 8 + 8;
const SEALED_LEN: usize = NONCE_LEN + PLAIN_LEN + 16;
const AAD_LABEL: &[u8] = b"sylvode.oauth.feed.cursor.v1";
const KEY_LABEL: &[u8] = b"sylvode.oauth.feed.cursor.key.v1";
const NONCE_LABEL: &[u8] = b"sylvode.oauth.feed.cursor.nonce.v1";

/// A position in the feed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Position {
    /// Sequence of the last event applied; `0` is the start of the feed.
    pub seq: i64,
    /// Unix seconds at which that event was written to the feed.
    pub at: i64,
}

/// Why a presented cursor was refused. The client is told only that the cursor is invalid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorError {
    Malformed,
    /// The cursor does not open under this grant: forged, damaged, or taken from another grant.
    Invalid,
}

/// The keys cursors are sealed with.
pub struct CursorKey {
    seal: LessSafeKey,
    nonce: hmac::Key,
}

impl fmt::Debug for CursorKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CursorKey([REDACTED])")
    }
}

fn derive(secret: &[u8], label: &[u8]) -> Vec<u8> {
    hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, secret), label)
        .as_ref()
        .to_vec()
}

impl CursorKey {
    /// Derives the keys from the instance's signing secret. `None` only if the cipher refuses the
    /// derived key, which a 32-byte value never does.
    pub fn derive(secret: &str) -> Option<Self> {
        let key = derive(secret.as_bytes(), KEY_LABEL);
        let nonce = derive(secret.as_bytes(), NONCE_LABEL);
        let unbound = UnboundKey::new(&AES_256_GCM, &key).ok()?;
        Some(Self {
            seal: LessSafeKey::new(unbound),
            nonce: hmac::Key::new(hmac::HMAC_SHA256, &nonce),
        })
    }

    fn plain(position: Position) -> [u8; PLAIN_LEN] {
        let mut out = [0_u8; PLAIN_LEN];
        out[0] = VERSION;
        out[1..9].copy_from_slice(&position.seq.to_be_bytes());
        out[9..].copy_from_slice(&position.at.to_be_bytes());
        out
    }

    fn nonce_for(&self, grant_id: Uuid, plain: &[u8]) -> [u8; NONCE_LEN] {
        let mut input = Vec::with_capacity(16 + plain.len());
        input.extend_from_slice(grant_id.as_bytes());
        input.extend_from_slice(plain);
        let tag = hmac::sign(&self.nonce, &input);
        let mut nonce = [0_u8; NONCE_LEN];
        for (slot, byte) in nonce.iter_mut().zip(tag.as_ref()) {
            *slot = *byte;
        }
        nonce
    }

    fn aad(grant_id: Uuid) -> Vec<u8> {
        let mut aad = Vec::with_capacity(AAD_LABEL.len() + 16);
        aad.extend_from_slice(AAD_LABEL);
        aad.extend_from_slice(grant_id.as_bytes());
        aad
    }

    /// The opaque text for `position` as seen by `grant_id`. `None` only if sealing failed.
    pub fn encode(&self, grant_id: Uuid, position: Position) -> Option<String> {
        let plain = Self::plain(position);
        let nonce_bytes = self.nonce_for(grant_id, &plain);
        let mut buffer = plain.to_vec();
        self.seal
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(Self::aad(grant_id)),
                &mut buffer,
            )
            .ok()?;
        let mut out = Vec::with_capacity(SEALED_LEN);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&buffer);
        Some(URL_SAFE_NO_PAD.encode(out))
    }

    /// Opens a presented cursor for `grant_id`.
    pub fn decode(&self, grant_id: Uuid, text: &str) -> Result<Position, CursorError> {
        if text.len() > 128 {
            return Err(CursorError::Malformed);
        }
        let raw = URL_SAFE_NO_PAD.decode(text).map_err(|_| CursorError::Malformed)?;
        if raw.len() != SEALED_LEN {
            return Err(CursorError::Malformed);
        }
        let (nonce_bytes, sealed) = raw.split_at_checked(NONCE_LEN).ok_or(CursorError::Malformed)?;
        let nonce = Nonce::try_assume_unique_for_key(nonce_bytes).map_err(|_| CursorError::Malformed)?;
        let mut buffer = sealed.to_vec();
        let plain = self
            .seal
            .open_in_place(nonce, Aad::from(Self::aad(grant_id)), &mut buffer)
            .map_err(|_| CursorError::Invalid)?;
        let (version, rest) = plain.split_first().ok_or(CursorError::Invalid)?;
        if *version != VERSION {
            return Err(CursorError::Invalid);
        }
        let (seq_bytes, at_bytes) = rest.split_at_checked(8).ok_or(CursorError::Invalid)?;
        let seq = i64::from_be_bytes(seq_bytes.try_into().map_err(|_| CursorError::Invalid)?);
        let at = i64::from_be_bytes(at_bytes.try_into().map_err(|_| CursorError::Invalid)?);
        if seq < 0 {
            return Err(CursorError::Invalid);
        }
        Ok(Position { seq, at })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn key() -> CursorKey {
        CursorKey::derive("a-signing-secret-for-cursor-tests-0123456789").expect("key")
    }

    #[test]
    fn a_position_round_trips_and_is_stable() {
        let grant = Uuid::new_v4();
        let key = key();
        let position = Position {
            seq: 18_342,
            at: 1_795_000_000,
        };
        let text = key.encode(grant, position).expect("sealed");
        assert_eq!(key.decode(grant, &text), Ok(position));
        // The same position always seals to the same id: events de-duplicate on it.
        assert_eq!(key.encode(grant, position).as_deref(), Some(text.as_str()));
        // Another position seals to another text.
        let next = key
            .encode(
                grant,
                Position {
                    seq: 18_343,
                    ..position
                },
            )
            .expect("sealed");
        assert_ne!(next, text);
    }

    #[test]
    fn the_text_is_encrypted_not_encoded() {
        let grant = Uuid::new_v4();
        let key = key();
        for seq in 1_i64..=5 {
            let position = Position { seq, at: 1_795_000_000 };
            let text = key.encode(grant, position).expect("sealed");
            assert_eq!(text.len(), 60, "{text}");
            let raw = URL_SAFE_NO_PAD.decode(&text).expect("base64");
            let sealed = &raw[NONCE_LEN..];
            let plain = CursorKey::plain(position);
            assert_ne!(
                &sealed[..PLAIN_LEN],
                &plain[..],
                "the position is not readable from the id"
            );
            assert!(
                !raw.windows(8).any(|w| w == seq.to_be_bytes()),
                "the sequence bytes appear in the id"
            );
        }
    }

    #[test]
    fn a_cursor_of_another_grant_does_not_open() {
        let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
        let key = key();
        let text = key.encode(theirs, Position { seq: 7, at: 1 }).expect("sealed");
        assert_eq!(key.decode(mine, &text), Err(CursorError::Invalid));
    }

    #[test]
    fn a_cursor_under_another_secret_does_not_open() {
        let grant = Uuid::new_v4();
        let text = key().encode(grant, Position { seq: 7, at: 1 }).expect("sealed");
        let other = CursorKey::derive("a-different-signing-secret-0123456789abcd").expect("key");
        assert_eq!(other.decode(grant, &text), Err(CursorError::Invalid));
    }

    #[test]
    fn forged_damaged_and_oversized_values_are_refused() {
        let grant = Uuid::new_v4();
        let key = key();
        let good = key.encode(grant, Position { seq: 7, at: 1 }).expect("sealed");
        assert_eq!(key.decode(grant, ""), Err(CursorError::Malformed));
        assert_eq!(key.decode(grant, "18342"), Err(CursorError::Malformed));
        assert_eq!(key.decode(grant, "not base64 !!"), Err(CursorError::Malformed));
        assert_eq!(key.decode(grant, &"A".repeat(500)), Err(CursorError::Malformed));
        assert_eq!(key.decode(grant, &good[..good.len() - 4]), Err(CursorError::Malformed));
        // Flip one character of a valid cursor: the authentication tag no longer matches.
        let mut chars: Vec<char> = good.chars().collect();
        chars[20] = if chars[20] == 'A' { 'B' } else { 'A' };
        let damaged: String = chars.into_iter().collect();
        assert_eq!(key.decode(grant, &damaged), Err(CursorError::Invalid));
        // A well-formed value of the right length that was never sealed.
        let forged = URL_SAFE_NO_PAD.encode([7_u8; SEALED_LEN]);
        assert_eq!(key.decode(grant, &forged), Err(CursorError::Invalid));
    }

    #[test]
    fn debug_output_hides_the_key() {
        assert_eq!(format!("{:?}", key()), "CursorKey([REDACTED])");
    }
}
