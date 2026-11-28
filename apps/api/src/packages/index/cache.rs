//! In-memory views of the cache: the parsed documents of the configured sources, and a bounded LRU
//! of fetched images.
//!
//! The database row is the source of truth. A parsed document is kept per source and replaced when
//! the row's `fetched_at` moves; a read never fetches anything from the network (images are the
//! one exception, fetched on demand by the asset route).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use platform::config::IndexSource;
use platform::timestamp::Timestamp;
use sea_orm::ConnectionTrait;
use sv_index::{IndexDoc, Options, parse_index};

use super::store;
use crate::error::ApiError;

struct Loaded {
    url: String,
    fetched_at: Timestamp,
    doc: Arc<IndexDoc>,
}

fn docs() -> &'static RwLock<HashMap<String, Loaded>> {
    static DOCS: OnceLock<RwLock<HashMap<String, Loaded>>> = OnceLock::new();
    DOCS.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Parses a validated index body; a failure is a document the current rules no longer accept.
pub(super) async fn parse_body(bytes: Vec<u8>) -> Result<IndexDoc, String> {
    tokio::task::spawn_blocking(move || {
        parse_index(&bytes, &Options::lenient()).map_err(|error| {
            error
                .issues
                .iter()
                .take(3)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        })
    })
    .await
    .map_err(|_| "the document could not be parsed".to_string())?
}

/// The parsed document of `source`, `None` while no good copy of this URL exists.
pub(super) async fn load(db: &impl ConnectionTrait, source: &IndexSource) -> Result<Option<Arc<IndexDoc>>, ApiError> {
    let Some(meta) = store::cache_meta(db, &source.id).await? else {
        return Ok(None);
    };
    let Some(fetched_at) = meta.fetched_at.filter(|_| meta.url == source.url) else {
        return Ok(None);
    };
    if let Some(loaded) = docs().read().get(&source.id)
        && loaded.url == source.url
        && loaded.fetched_at == fetched_at
    {
        return Ok(Some(Arc::clone(&loaded.doc)));
    }
    let Some((body, fetched_at)) = store::cache_body(db, &source.id, &source.url).await? else {
        return Ok(None);
    };
    let doc = match parse_body(body).await {
        Ok(doc) => Arc::new(doc),
        Err(error) => {
            tracing::error!(source = %source.id, %error, "the cached plugin index no longer validates");
            return Ok(None);
        }
    };
    docs().write().insert(
        source.id.clone(),
        Loaded {
            url: source.url.clone(),
            fetched_at,
            doc: Arc::clone(&doc),
        },
    );
    Ok(Some(doc))
}

/// A cached image.
#[derive(Clone)]
pub(super) struct Image {
    pub(super) bytes: Arc<Vec<u8>>,
    pub(super) content_type: &'static str,
}

/// Total bytes of images held in memory.
const MAX_IMAGE_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// How long an image is served from memory before it is fetched again.
const IMAGE_TTL: Duration = Duration::from_hours(1);

struct Entry {
    image: Image,
    at: Instant,
}

struct Lru {
    entries: HashMap<String, Entry>,
    /// Least recently used first.
    order: VecDeque<String>,
    bytes: usize,
}

fn images() -> &'static Mutex<Lru> {
    static IMAGES: OnceLock<Mutex<Lru>> = OnceLock::new();
    IMAGES.get_or_init(|| {
        Mutex::new(Lru {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        })
    })
}

impl Lru {
    fn forget(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.image.bytes.len());
        }
        self.order.retain(|other| other != key);
    }
}

pub(super) fn image_get(key: &str) -> Option<Image> {
    let mut lru = images().lock();
    let fresh = lru.entries.get(key).map(|entry| entry.at.elapsed() <= IMAGE_TTL)?;
    if !fresh {
        lru.forget(key);
        return None;
    }
    lru.order.retain(|other| other != key);
    lru.order.push_back(key.to_owned());
    lru.entries.get(key).map(|entry| entry.image.clone())
}

pub(super) fn image_put(key: String, image: Image) {
    let mut lru = images().lock();
    lru.forget(&key);
    lru.bytes = lru.bytes.saturating_add(image.bytes.len());
    lru.order.push_back(key.clone());
    lru.entries.insert(
        key,
        Entry {
            image,
            at: Instant::now(),
        },
    );
    while lru.bytes > MAX_IMAGE_CACHE_BYTES {
        let Some(oldest) = lru.order.front().cloned() else {
            break;
        };
        lru.forget(&oldest);
    }
    drop(lru);
}
