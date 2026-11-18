//! Ephemeral personal Page presence across API instances. NOTIFY carries bounded fragments;
//! no presence row, accepted document update or business event is persisted.
use super::collab::{
    frame::{Frame, PresenceActor},
    limits,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::Mutex;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OnceCell, broadcast};
use uuid::Uuid;

const CHANNEL: &str = "sylvode_flow_presence_v1";
const FRAGMENT_CHARS: usize = 6000;
const EVENTS_MAX: usize = 4096;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub object: Uuid,
    pub document: Uuid,
    pub session: Uuid,
    pub user: Uuid,
    pub payload: Value,
    pub ttl: u32,
}
impl Event {
    pub fn frame(&self) -> Frame {
        Frame::Presence {
            protocol_version: 1,
            document_id: self.document,
            session_id: self.session,
            payload: self.payload.clone(),
            ttl_seconds: Some(self.ttl),
            actor: Some(PresenceActor { user_id: self.user }),
        }
    }
}
#[derive(Serialize, Deserialize)]
struct Fragment {
    id: Uuid,
    index: usize,
    total: usize,
    bytes: String,
}
struct Entry {
    event: Event,
    expires: Instant,
}
pub struct Hub {
    sender: broadcast::Sender<Event>,
    entries: Mutex<HashMap<(Uuid, Uuid), Entry>>,
    ready: AtomicBool,
}
static HUB: OnceCell<Arc<Hub>> = OnceCell::const_new();
impl Hub {
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
    pub fn snapshot(&self, document: Uuid) -> Vec<Event> {
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| entry.expires > Instant::now());
        entries
            .values()
            .filter(|entry| entry.event.document == document)
            .map(|entry| {
                let mut event = entry.event.clone();
                event.ttl = u32::try_from(entry.expires.saturating_duration_since(Instant::now()).as_secs())
                    .unwrap_or(1)
                    .max(1);
                event
            })
            .collect()
    }
    fn accept(&self, event: Event) {
        let mut entries = self.entries.lock();
        entries.retain(|_, entry| entry.expires > Instant::now());
        let key = (event.document, event.session);
        if event.ttl == 0 {
            entries.remove(&key);
        } else {
            if !entries.contains_key(&key)
                && (entries.len() >= EVENTS_MAX
                    || entries.keys().filter(|(doc, _)| *doc == event.document).count()
                        >= limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX)
            {
                return;
            }
            entries.insert(
                key,
                Entry {
                    event: event.clone(),
                    expires: Instant::now() + Duration::from_secs(u64::from(event.ttl)),
                },
            );
        }
        drop(entries);
        let _ = self.sender.send(event);
    }
}
pub async fn start(db: &DatabaseConnection) -> Result<Arc<Hub>, crate::error::ApiError> {
    let hub = HUB
        .get_or_try_init(|| async {
            let mut listener = sea_orm::sqlx::postgres::PgListener::connect_with(db.get_postgres_connection_pool())
                .await
                .map_err(|_| crate::error::ApiError::internal())?;
            listener
                .listen(CHANNEL)
                .await
                .map_err(|_| crate::error::ApiError::internal())?;
            let (sender, _) = broadcast::channel(256);
            let hub = Arc::new(Hub {
                sender,
                entries: Mutex::new(HashMap::new()),
                ready: AtomicBool::new(true),
            });
            let task_hub = hub.clone();
            let task_db = db.clone();
            tokio::spawn(async move {
                let mut fragments = Assembler::default();
                loop {
                    if let Ok(notification) = listener.recv().await {
                        if let Some(event) = fragments.push(notification.payload()) {
                            task_hub.accept(event);
                        }
                        continue;
                    }
                    task_hub.ready.store(false, Ordering::Release);
                    task_hub.entries.lock().clear();
                    fragments = Assembler::default();
                    listener = reconnect(&task_db).await;
                    task_hub.ready.store(true, Ordering::Release);
                }
            });
            Ok::<_, crate::error::ApiError>(hub)
        })
        .await?;
    Ok(hub.clone())
}
#[derive(Default)]
struct Assembler {
    parts: HashMap<Uuid, (Instant, Vec<Option<String>>)>,
}
impl Assembler {
    fn push(&mut self, payload: &str) -> Option<Event> {
        self.parts
            .retain(|_, (created, _)| created.elapsed() < Duration::from_secs(5));
        let fragment: Fragment = serde_json::from_str(payload).ok()?;
        if fragment.total == 0
            || fragment.total > 4
            || fragment.index >= fragment.total
            || fragment.bytes.len() > FRAGMENT_CHARS
        {
            return None;
        }
        if !self.parts.contains_key(&fragment.id) && self.parts.len() >= 256 {
            return None;
        }
        let entry = self
            .parts
            .entry(fragment.id)
            .or_insert_with(|| (Instant::now(), vec![None; fragment.total]));
        if entry.1.len() != fragment.total {
            return None;
        }
        *entry.1.get_mut(fragment.index)? = Some(fragment.bytes);
        if !entry.1.iter().all(Option::is_some) {
            return None;
        }
        let encoded = entry.1.iter().filter_map(Option::as_deref).collect::<String>();
        self.parts.remove(&fragment.id);
        let bytes = STANDARD.decode(encoded).ok()?;
        let event: Event = serde_json::from_slice(&bytes).ok()?;
        (event.ttl <= limits::PRESENCE_TTL_SECONDS_MAX && valid_payload(&event.payload)).then_some(event)
    }
}
async fn reconnect(db: &DatabaseConnection) -> sea_orm::sqlx::postgres::PgListener {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Ok(mut fresh) =
            sea_orm::sqlx::postgres::PgListener::connect_with(db.get_postgres_connection_pool()).await
            && fresh.listen(CHANNEL).await.is_ok()
        {
            return fresh;
        }
    }
}
pub fn valid_payload(payload: &Value) -> bool {
    serde_json::to_vec(payload)
        .is_ok_and(|bytes| u64::try_from(bytes.len()).is_ok_and(|len| len <= limits::PRESENCE_PAYLOAD_BYTES_MAX))
}

pub async fn publish(db: &DatabaseConnection, event: Event) -> Result<(), crate::error::ApiError> {
    if event.ttl > limits::PRESENCE_TTL_SECONDS_MAX || !valid_payload(&event.payload) {
        return Err(crate::error::ApiError::invalid_update("invalid presence bounds"));
    }
    let encoded = STANDARD.encode(serde_json::to_vec(&event).map_err(|_| crate::error::ApiError::internal())?);
    let chunks: Vec<_> = encoded.as_bytes().chunks(FRAGMENT_CHARS).collect();
    let id = Uuid::new_v4();
    for (index, bytes) in chunks.iter().enumerate() {
        let fragment = Fragment {
            id,
            index,
            total: chunks.len(),
            bytes: String::from_utf8_lossy(bytes).into_owned(),
        };
        let bytes = serde_json::to_string(&fragment).map_err(|_| crate::error::ApiError::internal())?;
        db.execute(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_notify($1,$2)",
            vec![CHANNEL.into(), bytes.into()],
        ))
        .await?;
    }
    Ok(())
}
/// Closing a socket removes its ephemeral entry on every instance. Crashes expire by TTL.
pub struct Lease {
    pub db: DatabaseConnection,
    pub event: Event,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let db = self.db.clone();
        let mut event = self.event.clone();
        event.ttl = 0;
        event.payload = Value::Null;
        tokio::spawn(async move {
            let _ = publish(&db, event).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hub() -> Hub {
        let (sender, _) = broadcast::channel(256);
        Hub {
            sender,
            entries: Mutex::new(HashMap::new()),
            ready: AtomicBool::new(true),
        }
    }
    fn event(document: Uuid) -> Event {
        Event {
            object: Uuid::new_v4(),
            document,
            session: Uuid::new_v4(),
            user: Uuid::new_v4(),
            payload: serde_json::json!({"cursor":{"anchor":1}}),
            ttl: 30,
        }
    }
    #[test]
    fn bounded_ephemeral_cache_leave_and_attested_actor() {
        let hub = hub();
        let document = Uuid::new_v4();
        let mut receiver = hub.subscribe();
        let first = event(document);
        hub.accept(first.clone());
        assert_eq!(hub.snapshot(document).len(), 1);
        let frame = receiver.try_recv().unwrap().frame();
        assert!(
            matches!(frame,Frame::Presence {actor:Some(PresenceActor {user_id}),session_id,..} if user_id == first.user && session_id == first.session)
        );
        for _ in 1..limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX {
            hub.accept(event(document));
        }
        hub.accept(event(document));
        assert_eq!(hub.snapshot(document).len(), limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX);
        let mut leave = first;
        leave.ttl = 0;
        hub.accept(leave);
        assert_eq!(
            hub.snapshot(document).len(),
            limits::PRESENCE_ENTRIES_PER_DOCUMENT_MAX - 1
        );
    }
    #[test]
    fn maximum_protocol_payload_has_bounded_notify_fragments() {
        let mut event = event(Uuid::new_v4());
        event.payload = Value::String("中".repeat(2729));
        let payload = serde_json::to_vec(&event.payload).unwrap();
        assert!(u64::try_from(payload.len()).unwrap() <= limits::PRESENCE_PAYLOAD_BYTES_MAX);
        let encoded = STANDARD.encode(serde_json::to_vec(&event).unwrap());
        let chunks: Vec<_> = encoded.as_bytes().chunks(FRAGMENT_CHARS).collect();
        assert!(chunks.len() <= 4);
        for (index, chunk) in chunks.iter().enumerate() {
            let fragment = Fragment {
                id: Uuid::new_v4(),
                index,
                total: chunks.len(),
                bytes: String::from_utf8_lossy(chunk).into_owned(),
            };
            assert!(serde_json::to_string(&fragment).unwrap().len() < 8000);
        }
    }
}
