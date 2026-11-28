//! Read routes of the public plugin index (contract M15, section 3).
//!
//! They live under `/api/v1/workspaces/{workspace_id}/market/index`: sources and their status, the
//! plugin list with search and filters, a plugin's detail, and the plugin images.
//!
//! Every read comes from the cache; none fetches an index. Images are fetched on demand, only for
//! a path the cached document names (the icon or a screenshot), and served from a bounded memory
//! cache. Members read; a bot of the whole workspace reads the JSON routes (a project-scoped bot is
//! refused by `Caller`); the image route is for a console session.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use axum::{
    Extension,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use platform::{app::AppState, auth::JwtClaims, config::IndexSource, timestamp::Timestamp};
use semver::{Version, VersionReq};
use serde::Deserialize;
use serde_json::{Value, json};
use sv_index::{
    I18nText, ImageFormat, ImageRole, IndexDoc, PluginDoc, PublisherDoc, VersionEntry, VersionStatus, check_image,
    limits,
};
use tokio::sync::Semaphore;
use uuid::Uuid;

use super::{DEFAULT_PER_PAGE, MAX_PER_PAGE, cache, fetch, store};
use crate::{
    error::ApiError,
    middleware::bot_auth::BotAuthContext,
    packages::access::Caller,
    response::{ApiResponse, PageQuery, PaginatedData},
};

const KINDS: [&str; 4] = ["form-template", "page-template", "app", "plugin"];
/// Concurrent image fetches of the whole process.
const IMAGE_FETCHES: usize = 4;

pub(super) const fn kind_name(kind: sv_index::Kind) -> &'static str {
    match kind {
        sv_index::Kind::FormTemplate => "form-template",
        sv_index::Kind::PageTemplate => "page-template",
        sv_index::Kind::App => "app",
        sv_index::Kind::Plugin => "plugin",
    }
}

pub(super) fn coded(status: i32, code: &'static str, message: &str) -> ApiError {
    ApiError::Coded {
        status,
        code,
        message: message.to_string(),
        details: None,
    }
}

pub(super) fn source_not_found() -> ApiError {
    coded(
        404,
        "index_source_not_found",
        "the plugin index source does not exist or is hidden",
    )
}

pub(super) fn plugin_not_found() -> ApiError {
    coded(404, "index_plugin_not_found", "the plugin is not in the cached index")
}

/// The configured sources the workspace may see: the configured list minus the hidden ones.
async fn visible_sources(state: &AppState, workspace_id: Uuid) -> Result<Vec<IndexSource>, ApiError> {
    let hidden = store::hidden_sources(&state.db, workspace_id).await?;
    Ok(crate::config::runtime()
        .packages
        .index_sources
        .iter()
        .filter(|source| !hidden.contains(&source.id))
        .cloned()
        .collect())
}

/// The visible source `id`, or the refusal of an unknown or hidden one.
pub(super) async fn visible_source(state: &AppState, workspace_id: Uuid, id: &str) -> Result<IndexSource, ApiError> {
    visible_sources(state, workspace_id)
        .await?
        .into_iter()
        .find(|source| source.id == id)
        .ok_or_else(source_not_found)
}

/// The Sylvode version a plugin's `sylvode` range is compared with: this build, without a
/// pre-release tag (a range cannot name one).
pub(super) fn running_version() -> Option<Version> {
    static RUNNING: OnceLock<Option<Version>> = OnceLock::new();
    RUNNING
        .get_or_init(|| {
            let version = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
            Some(Version::new(version.major, version.minor, version.patch))
        })
        .clone()
}

pub(super) fn is_compatible(version: &VersionEntry) -> bool {
    let (Some(running), Ok(range)) = (running_version(), sv_index::parse_compat(&version.sylvode)) else {
        return false;
    };
    VersionReq::matches(&range, &running)
}

/// The version a client should offer: `channels.latest` when it is listed and compatible, else the
/// highest listed compatible version.
fn latest_compatible(plugin: &PluginDoc) -> Option<&VersionEntry> {
    let usable = |version: &&VersionEntry| version.status == VersionStatus::Listed && is_compatible(version);
    if let Some(latest) = plugin.channels.latest.as_deref()
        && let Some(version) = plugin
            .versions
            .iter()
            .find(|version| version.version == latest)
            .filter(usable)
    {
        return Some(version);
    }
    plugin
        .versions
        .iter()
        .filter(usable)
        .max_by_key(|version| Version::parse(&version.version).ok())
}

fn status_of(meta: Option<&store::CacheMeta>, refresh_secs: u64) -> (bool, Option<Timestamp>) {
    let Some(meta) = meta else {
        return (true, None);
    };
    let waited = Timestamp::now()
        .unix_timestamp()
        .saturating_sub(meta.checked_at.unix_timestamp());
    let overdue = u64::try_from(waited).is_ok_and(|waited| waited > refresh_secs.saturating_mul(3));
    (
        meta.fetched_at.is_none() || meta.error.is_some() || overdue,
        meta.fetched_at,
    )
}

/// A source URL as members and bots see it: without a query or fragment, which may carry a token
/// the operator put there (the fetch uses the full URL).
fn display_url(url: &str) -> String {
    reqwest::Url::parse(url).map_or_else(
        |_| String::new(),
        |mut url| {
            url.set_query(None);
            url.set_fragment(None);
            url.into()
        },
    )
}

/// The status object of one configured source.
fn source_json(source: &IndexSource, hidden: bool, meta: Option<store::CacheMeta>, refresh_secs: u64) -> Value {
    // A copy of another URL (the operator changed the source) is not this source's copy.
    let meta = meta.filter(|meta| meta.url == source.url);
    let (is_stale, fetched_at) = status_of(meta.as_ref(), refresh_secs);
    json!({
        "id": source.id,
        "url": display_url(&source.url),
        "hidden": hidden,
        "fetched_at": fetched_at,
        "checked_at": meta.as_ref().map(|meta| meta.checked_at),
        "generated_at": meta.as_ref().and_then(|meta| meta.generated_at),
        "stale": is_stale,
        "error": meta.as_ref().and_then(|meta| meta.error.clone()),
        "warnings": meta.as_ref().map_or(Value::Array(Vec::new()), |meta| meta.warnings.clone()),
        "plugins": meta.as_ref().filter(|meta| meta.fetched_at.is_some()).map_or(0, |meta| meta.plugin_count),
    })
}

/// The status of `source` for the workspace, read from the database now.
pub(super) async fn source_status(
    state: &AppState,
    workspace_id: Uuid,
    source: &IndexSource,
) -> Result<Value, ApiError> {
    let hidden = store::hidden_sources(&state.db, workspace_id).await?;
    let meta = store::cache_meta(&state.db, &source.id).await?;
    Ok(source_json(
        source,
        hidden.contains(&source.id),
        meta,
        crate::config::runtime().packages.index_refresh_secs,
    ))
}

/// `GET /sources`: every configured source with its status, hidden ones included.
pub async fn list_sources(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<impl IntoResponse, ApiError> {
    Caller::resolve(&state, &claims, bot, workspace_id).await?;
    let hidden = store::hidden_sources(&state.db, workspace_id).await?;
    let config = &crate::config::runtime().packages;
    let mut sources = Vec::with_capacity(config.index_sources.len());
    for source in &config.index_sources {
        let meta = store::cache_meta(&state.db, &source.id).await?;
        sources.push(source_json(
            source,
            hidden.contains(&source.id),
            meta,
            config.index_refresh_secs,
        ));
    }
    Ok(ApiResponse::success(sources))
}

#[derive(Debug, Deserialize)]
pub struct PluginsQuery {
    source: Option<String>,
    q: Option<String>,
    kind: Option<String>,
    category: Option<String>,
    label: Option<String>,
    compatible: Option<bool>,
    page: Option<i64>,
    per_page: Option<i64>,
}

/// A plugin of a loaded source with the facts the list needs.
struct Listed<'a> {
    source: &'a IndexSource,
    doc: &'a IndexDoc,
    plugin: &'a PluginDoc,
}

impl Listed<'_> {
    fn publisher(&self) -> Option<&PublisherDoc> {
        self.doc.publishers.get(plugin_publisher(&self.plugin.id))
    }
}

pub(super) fn plugin_publisher(plugin_id: &str) -> &str {
    plugin_id.split_once('/').map_or(plugin_id, |(publisher, _)| publisher)
}

fn text_has(text: &I18nText, needle: &str) -> bool {
    text.values().any(|value| value.to_lowercase().contains(needle))
}

fn matches_query(listed: &Listed<'_>, needle: &str) -> bool {
    let plugin = listed.plugin;
    plugin.id.to_lowercase().contains(needle)
        || text_has(&plugin.title, needle)
        || text_has(&plugin.summary, needle)
        || plugin
            .metadata
            .keywords
            .iter()
            .any(|keyword| keyword.to_lowercase().contains(needle))
        || listed
            .publisher()
            .is_some_and(|publisher| text_has(&publisher.name, needle))
}

/// `GET /plugins`: the cached plugins of the visible sources, filtered and paged.
pub async fn list_plugins(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path(workspace_id): Path<Uuid>,
    Query(query): Query<PluginsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    Caller::resolve(&state, &claims, bot, workspace_id).await?;
    if let Some(kind) = query.kind.as_deref()
        && !KINDS.contains(&kind)
    {
        return Err(ApiError::bad_request_coded(
            "kind_invalid",
            "kind must be one of: form-template, page-template, app, plugin",
        ));
    }
    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_lowercase);
    if needle.as_ref().is_some_and(|q| q.chars().count() > 100) {
        return Err(ApiError::bad_request_coded(
            "q_invalid",
            "q is limited to 100 characters",
        ));
    }
    let sources = match query.source.as_deref() {
        Some(id) => vec![visible_source(&state, workspace_id, id).await?],
        None => visible_sources(&state, workspace_id).await?,
    };
    let mut loaded: Vec<(IndexSource, Arc<IndexDoc>)> = Vec::new();
    for source in sources {
        if let Some(doc) = cache::load(&state.db, &source).await? {
            loaded.push((source, doc));
        }
    }
    let compatible_only = query.compatible.unwrap_or(false);
    let rows: Vec<Listed<'_>> = loaded
        .iter()
        .flat_map(|(source, doc)| {
            doc.plugins.iter().map(move |plugin| Listed {
                source,
                doc: doc.as_ref(),
                plugin,
            })
        })
        .filter(|listed| {
            query
                .kind
                .as_deref()
                .is_none_or(|kind| kind_name(listed.plugin.kind) == kind)
                && query
                    .category
                    .as_deref()
                    .is_none_or(|category| listed.plugin.metadata.category == category)
                && query
                    .label
                    .as_deref()
                    .is_none_or(|label| listed.plugin.metadata.labels.iter().any(|other| other == label))
                && (!compatible_only || latest_compatible(listed.plugin).is_some())
                && needle.as_deref().is_none_or(|needle| matches_query(listed, needle))
        })
        .collect();
    let paging = PageQuery {
        page: query.page,
        per_page: query.per_page,
    }
    .resolve(DEFAULT_PER_PAGE, MAX_PER_PAGE);
    let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
    let start = usize::try_from(paging.offset()).unwrap_or(usize::MAX);
    let page: Vec<&Listed<'_>> = rows
        .iter()
        .skip(start)
        .take(usize::try_from(paging.per_page).unwrap_or(0))
        .collect();
    let ids: Vec<String> = page.iter().map(|listed| listed.plugin.id.clone()).collect();
    let publishers: Vec<String> = page
        .iter()
        .map(|listed| plugin_publisher(&listed.plugin.id).to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let held = store::workspace_state(&state.db, workspace_id, &ids, &publishers).await?;
    let items: Vec<Value> = page
        .iter()
        .map(|listed| {
            let plugin = listed.plugin;
            let publisher_id = plugin_publisher(&plugin.id);
            let publisher = listed.publisher();
            let digests: BTreeSet<&str> = plugin.versions.iter().map(|version| version.digest.as_str()).collect();
            let imported = held
                .packages
                .get(&plugin.id)
                .is_some_and(|have| have.iter().any(|digest| digests.contains(digest.as_str())));
            json!({
                "source": listed.source.id,
                "id": plugin.id,
                "kind": kind_name(plugin.kind),
                "title": plugin.title,
                "summary": plugin.summary,
                "icon": plugin.icon,
                "publisher": {
                    "id": publisher_id,
                    "name": publisher.map(|publisher| &publisher.name),
                    "trusted": publisher_trusted(publisher, &held, publisher_id),
                },
                "category": plugin.metadata.category,
                "latest_compatible": latest_compatible(plugin).map(|version| &version.version),
                "labels": plugin.metadata.labels,
                "imported": imported,
                "installed": held.installed.contains(&plugin.id),
            })
        })
        .collect();
    Ok(ApiResponse::success(PaginatedData::page(items, total, paging)))
}

/// A publisher is trusted in the workspace when an active key it holds in the index is pinned there.
fn publisher_trusted(publisher: Option<&PublisherDoc>, held: &store::WorkspaceState, id: &str) -> bool {
    let (Some(publisher), Some(pinned)) = (publisher, held.keys.get(id)) else {
        return false;
    };
    publisher
        .keys
        .iter()
        .any(|key| key.status == sv_index::KeyStatus::Active && pinned.contains(&key.key_id))
}

/// `GET /plugins/{source}/{publisher}/{name}`: the cached plugin document with compatibility and
/// import state per version and the publisher's keys with their pinned state.
pub async fn get_plugin(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    bot: Option<Extension<BotAuthContext>>,
    Path((workspace_id, source_id, publisher_id, name)): Path<(Uuid, String, String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    Caller::resolve(&state, &claims, bot, workspace_id).await?;
    let source = visible_source(&state, workspace_id, &source_id).await?;
    let doc = cache::load(&state.db, &source).await?.ok_or_else(plugin_not_found)?;
    let plugin_id = format!("{publisher_id}/{name}");
    let plugin = doc
        .plugins
        .iter()
        .find(|plugin| plugin.id == plugin_id)
        .ok_or_else(plugin_not_found)?;
    let publisher = doc.publishers.get(&publisher_id);
    let held = store::workspace_state(
        &state.db,
        workspace_id,
        std::slice::from_ref(&plugin_id),
        std::slice::from_ref(&publisher_id),
    )
    .await?;
    let have = held.packages.get(&plugin_id);
    let digests: BTreeSet<&str> = plugin.versions.iter().map(|version| version.digest.as_str()).collect();
    let mut body = serde_json::to_value(plugin).map_err(|_| ApiError::internal())?;
    if let Some(versions) = body.get_mut("versions").and_then(Value::as_array_mut) {
        for (value, version) in versions.iter_mut().zip(&plugin.versions) {
            if let Some(object) = value.as_object_mut() {
                object.insert("compatible".into(), json!(is_compatible(version)));
                let imported = have.is_some_and(|have| have.contains(&version.digest));
                object.insert(
                    "imported_digest".into(),
                    if imported { json!(version.digest) } else { Value::Null },
                );
            }
        }
    }
    let pinned = held.keys.get(&publisher_id);
    let keys: Vec<Value> = publisher.map_or_else(Vec::new, |publisher| {
        publisher
            .keys
            .iter()
            .map(|key| {
                let mut value = serde_json::to_value(key).unwrap_or(Value::Null);
                if let Some(object) = value.as_object_mut() {
                    object.insert(
                        "pinned".into(),
                        json!(pinned.is_some_and(|pinned| pinned.contains(&key.key_id))),
                    );
                }
                value
            })
            .collect()
    });
    if let Some(object) = body.as_object_mut() {
        object.insert("kind".into(), json!(kind_name(plugin.kind)));
        object.insert("source".into(), json!(source.id));
        object.insert("source_url".into(), json!(display_url(&source.url)));
        object.insert(
            "publisher".into(),
            json!({
                "id": publisher_id,
                "name": publisher.map(|publisher| &publisher.name),
                "homepage": publisher.and_then(|publisher| publisher.homepage.clone()),
                "trusted": publisher_trusted(publisher, &held, &publisher_id),
                "keys": keys,
            }),
        );
        object.insert(
            "latest_compatible".into(),
            json!(latest_compatible(plugin).map(|version| &version.version)),
        );
        object.insert(
            "imported".into(),
            json!(have.is_some_and(|have| have.iter().any(|digest| digests.contains(digest.as_str())))),
        );
        object.insert("installed".into(), json!(held.installed.contains(&plugin_id)));
    }
    Ok(ApiResponse::success(body))
}

fn image_fetches() -> &'static Semaphore {
    static SLOTS: OnceLock<Semaphore> = OnceLock::new();
    SLOTS.get_or_init(|| Semaphore::new(IMAGE_FETCHES))
}

const fn content_type(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::Webp => "image/webp",
    }
}

/// The URL of a file of a plugin directory: the index URL with its last segment replaced by
/// `plugins/<publisher>/<name>/<path>`. `None` for a path with an empty, `.` or `..` segment.
fn asset_url(index_url: &str, publisher: &str, name: &str, path: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(index_url).ok()?;
    url.set_query(None);
    url.set_fragment(None);
    {
        let mut segments = url.path_segments_mut().ok()?;
        segments.pop().push("plugins").push(publisher).push(name);
        for part in path.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return None;
            }
            segments.push(part);
        }
    }
    Some(url.into())
}

fn image_response(image: &cache::Image) -> Response {
    let mut response = Response::new(Body::from(image.bytes.as_ref().clone()));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(image.content_type));
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("sandbox"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=600"));
    response
}

/// `GET /assets/{source}/{publisher}/{name}/{*asset_path}`: the icon or a screenshot of a cached
/// plugin. Any other path is refused before anything is fetched.
pub async fn get_asset(
    State(state): State<AppState>,
    Extension(claims): Extension<JwtClaims>,
    Path((workspace_id, source_id, publisher_id, name, asset_path)): Path<(Uuid, String, String, String, String)>,
) -> Result<Response, ApiError> {
    Caller::resolve(&state, &claims, None, workspace_id).await?;
    let source = visible_source(&state, workspace_id, &source_id).await?;
    let doc = cache::load(&state.db, &source).await?.ok_or_else(plugin_not_found)?;
    let plugin_id = format!("{publisher_id}/{name}");
    let plugin = doc
        .plugins
        .iter()
        .find(|plugin| plugin.id == plugin_id)
        .ok_or_else(plugin_not_found)?;
    let role = if plugin.icon == asset_path {
        ImageRole::Icon
    } else if plugin.screenshots.iter().any(|shot| shot.path == asset_path) {
        ImageRole::Screenshot
    } else {
        return Err(coded(404, "index_asset_not_found", "the plugin has no such image"));
    };
    let key = format!("{}\n{}\n{}", source.id, plugin_id, asset_path);
    if let Some(image) = cache::image_get(&key) {
        return Ok(image_response(&image));
    }
    let url = asset_url(&source.url, &publisher_id, &name, &asset_path)
        .ok_or_else(|| coded(404, "index_asset_not_found", "the plugin has no such image"))?;
    let limit = match role {
        ImageRole::Icon => limits::MAX_ICON_BYTES,
        ImageRole::Screenshot => limits::MAX_SCREENSHOT_BYTES,
    };
    let unavailable = || {
        coded(
            502,
            "index_asset_unavailable",
            "the image could not be fetched from the index source",
        )
    };
    let _slot = image_fetches().acquire().await.map_err(|_| ApiError::internal())?;
    let bytes = match fetch::get(&url, limit, None, "image/png, image/jpeg, image/webp").await {
        Ok(fetch::Answer::Body { bytes, .. }) => bytes,
        Ok(fetch::Answer::NotModified) | Err(_) => return Err(unavailable()),
    };
    // The same rules as the repository's CI: magic bytes, size and dimensions per role.
    let info = check_image(role, Some(&asset_path), &bytes).map_err(|error| {
        tracing::warn!(source = %source.id, plugin = %plugin_id, %error, "a plugin image broke the image rules");
        unavailable()
    })?;
    let image = cache::Image {
        bytes: Arc::new(bytes),
        content_type: content_type(info.format),
    };
    cache::image_put(key, image.clone());
    Ok(image_response(&image))
}
