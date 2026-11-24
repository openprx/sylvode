//! The workspace a networked caller acts in, taken from the caller's own bot token.
//!
//! One MCP server serves any workspace: a request that carries a workspace bot token acts in the
//! workspace that token belongs to, which the API reports on `GET /api/v1/auth/me`
//! (`data.bot.workspace_id`). The answer is cached per token for a short time so that a burst of
//! calls costs one lookup. The cache is keyed by the SHA-256 of the token, never by the token, and
//! holds no failure: a token the API refused is asked about again on the next call.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

/// How long a resolved workspace is trusted. A token that is revoked or moved keeps acting for at
/// most this long, and every call the API itself receives is still authenticated by the API.
pub const WORKSPACE_CACHE_TTL: Duration = Duration::from_mins(1);

/// Most tokens the cache holds at once. When it is full the oldest entry makes room.
pub const WORKSPACE_CACHE_CAPACITY: usize = 1_000;

/// What a caller whose token belongs to no bot is told. The `401` is the status such a token gets
/// from every other workspace-scoped route of a bot-only door.
pub const NOT_A_BOT_TOKEN: &str = "API error 401: this credential belongs to no workspace bot, so the workspace to act in cannot be taken \
     from it; present a workspace bot token";

struct Entry {
    workspace_id: String,
    stored_at: Instant,
}

/// Workspace by token hash, bounded in size and in age.
pub struct WorkspaceCache {
    lifetime: Duration,
    capacity: usize,
    entries: Mutex<HashMap<[u8; 32], Entry>>,
}

impl WorkspaceCache {
    pub fn new(lifetime: Duration, capacity: usize) -> Self {
        Self {
            lifetime,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    fn key(token: &str) -> [u8; 32] {
        Sha256::digest(token.as_bytes()).into()
    }

    /// The workspace stored for `token`, unless it is older than the lifetime.
    pub async fn get(&self, token: &str) -> Option<String> {
        let key = Self::key(token);
        let mut entries = self.entries.lock().await;
        match entries.get(&key) {
            Some(entry) if entry.stored_at.elapsed() < self.lifetime => Some(entry.workspace_id.clone()),
            Some(_) => {
                entries.remove(&key);
                None
            }
            None => None,
        }
    }

    pub async fn put(&self, token: &str, workspace_id: String) {
        let key = Self::key(token);
        let mut entries = self.entries.lock().await;
        if entries.len() >= self.capacity && !entries.contains_key(&key) {
            let lifetime = self.lifetime;
            entries.retain(|_, entry| entry.stored_at.elapsed() < lifetime);
            if entries.len() >= self.capacity {
                let oldest = entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.stored_at)
                    .map(|(oldest, _)| *oldest);
                if let Some(oldest) = oldest {
                    entries.remove(&oldest);
                }
            }
        }
        entries.insert(
            key,
            Entry {
                workspace_id,
                stored_at: Instant::now(),
            },
        );
    }

    #[cfg(test)]
    pub async fn len(&self) -> usize {
        self.entries.lock().await.len()
    }
}

/// The workspace of the bot an `auth/me` answer describes.
///
/// `Err` carries the text a tool call fails with: the token names no bot, or the answer does not
/// carry a workspace id.
pub fn workspace_of_me(me: &Value) -> Result<String, String> {
    let data = me.get("data").unwrap_or(me);
    let bot = data.get("bot").filter(|bot| !bot.is_null()).ok_or(NOT_A_BOT_TOKEN)?;
    bot.get("workspace_id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .map(|id| id.to_string())
        .ok_or_else(|| "Malformed response from /api/v1/auth/me: the bot carries no workspace_id".to_string())
}

#[cfg(test)]
mod tests {
    use super::{NOT_A_BOT_TOKEN, WorkspaceCache, workspace_of_me};
    use serde_json::json;
    use std::time::Duration;

    const WORKSPACE: &str = "3a9f5c42-81de-4e07-b6a1-5c2e90d47f38";

    #[test]
    fn a_bot_answer_names_its_workspace() {
        let me =
            json!({"code":0,"message":"ok","data":{"user":{},"bot":{"bot_id":WORKSPACE,"workspace_id":WORKSPACE}}});
        assert_eq!(workspace_of_me(&me).as_deref(), Ok(WORKSPACE));
    }

    #[test]
    fn a_person_or_a_malformed_answer_names_none() {
        let person = json!({"code":0,"message":"ok","data":{"user":{},"bot":null}});
        assert_eq!(workspace_of_me(&person), Err(NOT_A_BOT_TOKEN.to_string()));
        let absent = json!({"code":0,"message":"ok","data":{"user":{}}});
        assert_eq!(workspace_of_me(&absent), Err(NOT_A_BOT_TOKEN.to_string()));
        let broken = json!({"code":0,"message":"ok","data":{"bot":{"workspace_id":"nope"}}});
        assert!(workspace_of_me(&broken).is_err_and(|error| error.contains("workspace_id")));
    }

    #[tokio::test]
    async fn an_entry_expires_after_the_lifetime() {
        let cache = WorkspaceCache::new(Duration::from_millis(40), 8);
        cache.put("token-a", WORKSPACE.to_string()).await;
        assert_eq!(cache.get("token-a").await.as_deref(), Some(WORKSPACE));
        assert_eq!(cache.get("token-b").await, None);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(cache.get("token-a").await, None);
        assert_eq!(cache.len().await, 0, "an expired entry is dropped when it is found");
    }

    #[tokio::test]
    async fn the_cache_never_grows_past_its_capacity_and_drops_the_oldest() {
        let cache = WorkspaceCache::new(Duration::from_mins(1), 3);
        for index in 0..5 {
            cache.put(&format!("token-{index}"), format!("workspace-{index}")).await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert_eq!(cache.len().await, 3);
        assert_eq!(cache.get("token-0").await, None);
        assert_eq!(cache.get("token-1").await, None);
        assert_eq!(cache.get("token-4").await.as_deref(), Some("workspace-4"));
        // Storing a token that is already there replaces it and evicts nobody.
        cache.put("token-4", "workspace-new".to_string()).await;
        assert_eq!(cache.len().await, 3);
        assert_eq!(cache.get("token-2").await.as_deref(), Some("workspace-2"));
    }

    #[tokio::test]
    async fn a_full_cache_prefers_to_drop_expired_entries() {
        let cache = WorkspaceCache::new(Duration::from_millis(30), 2);
        cache.put("old-a", "a".to_string()).await;
        cache.put("old-b", "b".to_string()).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        cache.put("fresh", "c".to_string()).await;
        assert_eq!(cache.len().await, 1);
    }
}
