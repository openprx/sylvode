//! Server-owned logical identity decisions. Raw CRDT attributes cannot nominate an identity.
//! Persistence must apply this decision under placement-generation CAS in the accepted-head tx.

use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AliasState {
    Active,
    Deleted,
    /// The logical resource has left this physical alias. This alias can never reclaim it.
    Moved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alias {
    pub logical_id: Uuid,
    pub generation: u64,
    pub state: AliasState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IdentityDecision {
    pub logical_id: Uuid,
    pub generation: u64,
    pub suspend_shares: bool,
    pub fresh_identity: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityError {
    GenerationOverflow,
    StalePlacement,
    FreshIdentityCollision,
}

/// `alias` is loaded by the server for exactly (Page, physical CID). `fresh` is server generated.
///
/// Deleted local aliases may restore their original identity, but never their old grants.
/// A moved alias remains an audit anchor; restoring source history creates a new resource.
pub fn reconcile_alias(alias: Option<Alias>, fresh: Uuid) -> Result<IdentityDecision, IdentityError> {
    match alias {
        Some(Alias {
            logical_id,
            generation,
            state: AliasState::Active,
        }) => Ok(IdentityDecision {
            logical_id,
            generation,
            suspend_shares: false,
            fresh_identity: false,
        }),
        Some(Alias {
            logical_id,
            generation,
            state: AliasState::Deleted,
        }) => Ok(IdentityDecision {
            logical_id,
            generation: generation.checked_add(1).ok_or(IdentityError::GenerationOverflow)?,
            suspend_shares: true,
            fresh_identity: false,
        }),
        old => {
            if fresh.is_nil() || old.is_some_and(|a| a.logical_id == fresh) {
                return Err(IdentityError::FreshIdentityCollision);
            }
            Ok(IdentityDecision {
                logical_id: fresh,
                generation: 1,
                suspend_shares: true,
                fresh_identity: true,
            })
        }
    }
}

/// Only a separately authorized two-Page transaction can invoke a move. Raw updates cannot.
///
/// The transaction retires the old alias and creates a new physical alias with this generation.
pub fn trusted_move(alias: Alias, expected_generation: u64) -> Result<IdentityDecision, IdentityError> {
    if alias.state != AliasState::Active || alias.generation != expected_generation {
        return Err(IdentityError::StalePlacement);
    }
    Ok(IdentityDecision {
        logical_id: alias.logical_id,
        generation: alias
            .generation
            .checked_add(1)
            .ok_or(IdentityError::GenerationOverflow)?,
        suspend_shares: true,
        fresh_identity: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias(state: AliasState) -> Alias {
        Alias {
            logical_id: Uuid::from_u128(1),
            generation: 8,
            state,
        }
    }

    #[test]
    fn ordinary_edits_keep_generation_and_local_restore_suspends_grants() {
        let active = reconcile_alias(Some(alias(AliasState::Active)), Uuid::from_u128(2)).unwrap();
        assert_eq!(active.generation, 8);
        assert!(!active.suspend_shares);
        let restored = reconcile_alias(Some(alias(AliasState::Deleted)), Uuid::from_u128(2)).unwrap();
        assert_eq!(restored.logical_id, active.logical_id);
        assert_eq!(restored.generation, 9);
        assert!(restored.suspend_shares);
    }

    #[test]
    fn source_history_cannot_reclaim_moved_identity() {
        let moved = trusted_move(alias(AliasState::Active), 8).unwrap();
        let restored = reconcile_alias(Some(alias(AliasState::Moved)), Uuid::from_u128(2)).unwrap();
        assert_ne!(restored.logical_id, moved.logical_id);
        assert!(restored.fresh_identity);
        assert!(restored.suspend_shares);
        assert_eq!(
            trusted_move(alias(AliasState::Moved), 8),
            Err(IdentityError::StalePlacement)
        );
        assert_eq!(
            trusted_move(alias(AliasState::Active), 7),
            Err(IdentityError::StalePlacement)
        );
    }

    #[test]
    fn fresh_ids_and_generation_cannot_silently_alias_or_wrap() {
        assert_eq!(
            reconcile_alias(None, Uuid::nil()),
            Err(IdentityError::FreshIdentityCollision)
        );
        assert_eq!(
            reconcile_alias(Some(alias(AliasState::Moved)), Uuid::from_u128(1)),
            Err(IdentityError::FreshIdentityCollision)
        );
        let exhausted = Alias {
            generation: u64::MAX,
            ..alias(AliasState::Active)
        };
        assert_eq!(
            trusted_move(exhausted, u64::MAX),
            Err(IdentityError::GenerationOverflow)
        );
    }
}
