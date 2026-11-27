//! Scope-native ticket admission and personal collaborative sessions. Persisted update polling
//! supplies cross-instance ordered fanout; every read/send pass rechecks current authorization.
#[derive(FromQueryResult)]
struct TicketIdentityMatch {
    matches: bool,
}

#[derive(FromQueryResult)]
struct TicketExpiry {
    expires_at: platform::timestamp::Timestamp,
}

use super::{
    collab::{
        self,
        authz::PermissionLevel,
        frame::{Frame, RejectedCode, TailUpdate, WriteState},
        limits,
        session::{HandshakePhase, read_handshake_frame},
    },
    scope, scope_write,
};
use crate::error::ApiError;
use crate::flow::json_value::FlowJson;
use axum::extract::ws::{CloseFrame, Message, WebSocket};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD},
};
use parking_lot::Mutex;
use platform::app::AppState;
use rand::RngCore;
use sea_orm::{DatabaseConnection, DbBackend, FromQueryResult, Statement, TransactionTrait};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::LazyLock, time::Duration};
use uuid::Uuid;
#[derive(FromQueryResult)]
struct PersistedFrame {
    seq: i64,
    update_id: Uuid,
    bytes: Vec<u8>,
    before_frontier: Vec<u8>,
    after_frontier: Vec<u8>,
    event_id: Uuid,
    origin_client_id: Option<String>,
}
#[derive(FromQueryResult)]
struct Document {
    id: Uuid,
    object_id: Uuid,
    scope_id: Uuid,
    workspace_id: Option<Uuid>,
    authz_revision: i64,
    user_id: Uuid,
    client_id: String,
    authorization_notifications: bool,
}
#[derive(Clone)]
pub struct TicketContext {
    pub user_id: Uuid,
    pub scope_id: Uuid,
    pub workspace_id: Option<Uuid>,
    pub object_id: Uuid,
    pub document_id: Uuid,
    pub client_id: String,
    pub authorization_notifications: bool,
}
fn digest(raw: &str) -> String {
    hex::encode(Sha256::digest(raw.as_bytes()))
}
pub async fn issue(
    db: &DatabaseConnection,
    user: Uuid,
    object: Uuid,
    document: Uuid,
    client: Uuid,
    raw_origin: &str,
    allowed: &[String],
) -> Result<Value, ApiError> {
    issue_with_options(db, user, object, document, client, raw_origin, allowed, false).await
}
#[allow(clippy::too_many_arguments)]
pub async fn issue_with_options(
    db: &DatabaseConnection,
    user: Uuid,
    object: Uuid,
    document: Uuid,
    client: Uuid,
    raw_origin: &str,
    allowed: &[String],
    authorization_notifications: bool,
) -> Result<Value, ApiError> {
    let client = client.to_string();
    let origin =
        collab::origin::normalize(raw_origin).ok_or_else(|| ApiError::invalid_update("invalid collab origin"))?;
    if !collab::origin::is_allowed(&origin, allowed) {
        return Err(ApiError::forbidden("origin is not allowlisted for collab"));
    }
    let proof = scope::authorize(db, object, user, PermissionLevel::View).await?;
    let identity = scope::identity(db, object).await?;
    if matches!(identity.object_type.as_str(), "collection" | "record") {
        return Err(ApiError::policy_rejected(
            "collection and record documents require server-only typed commands",
        ));
    }
    super::scope_editor::require_admitted(db, object).await?;
    let tx = db.begin().await?;
    scope::fence(&tx, &proof).await?;

    if !TicketIdentityMatch::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT EXISTS(SELECT 1 FROM collab_documents WHERE id=$1 AND object_id=$2) AS matches",
        vec![document.into(), object.into()],
    ))
    .one(&tx)
    .await?
    .is_some_and(|m| m.matches)
    {
        return Err(ApiError::not_found("document not found"));
    }
    let mut secret = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut secret);
    let ticket = URL_SAFE_NO_PAD.encode(secret);

    let expiry=TicketExpiry::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"INSERT INTO collab_tickets(ticket_hash,user_id,workspace_id,scope_id,authz_revision,document_id,client_id,origin,authorization_notifications,created_at,expires_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,now(),now()+interval '60 seconds') RETURNING expires_at",vec![digest(&ticket).into(),user.into(),identity.workspace_id.into(),identity.scope_id.into(),proof.revision.into(),document.into(),client.clone().into(),origin.into(),authorization_notifications.into()])).one(&tx).await?.ok_or_else(ApiError::internal)?;
    tx.commit().await?;
    let websocket_url = format!(
        "/api/v2/flow/collab/ws?ticket={ticket}&client_id={}",
        percent_encode(&client)
    );
    Ok(
        json!({"ticket":ticket,"expires_at":expiry.expires_at.to_rfc3339(),"websocket_path":websocket_url,"websocket_url":websocket_url,"scope_id":identity.scope_id,"object_id":object,"document_id":document,"authz_revision":proof.revision,"limits":scope_write::limits(),"authorization_notifications":authorization_notifications}),
    )
}
fn percent_encode(raw: &str) -> String {
    raw.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub async fn consume(
    db: &DatabaseConnection,
    ticket: &str,
    client: Uuid,
    origin: &str,
) -> Result<TicketContext, ApiError> {
    let client = client.to_string();
    let normalized = collab::origin::normalize(origin).ok_or_else(|| ApiError::unauthenticated("invalid ticket"))?;
    let tx = db.begin().await?;
    let row=Document::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"UPDATE collab_tickets t SET consumed_at=now() FROM collab_documents d WHERE t.ticket_hash=$1 AND t.client_id=$2 AND t.origin=$3 AND t.consumed_at IS NULL AND t.expires_at>now() AND t.document_id=d.id AND t.authz_revision IS NOT NULL RETURNING d.id,d.object_id,t.scope_id,t.workspace_id,t.authz_revision,t.user_id,t.client_id,t.authorization_notifications",vec![digest(ticket).into(),client.into(),normalized.into()])).one(&tx).await?.ok_or_else(||ApiError::unauthenticated("invalid ticket"))?;
    let proof = scope::authorize(&tx, row.object_id, row.user_id, PermissionLevel::View).await?;
    let scope_changed = proof.scope_id != row.scope_id;
    let revision_changed = proof.revision != row.authz_revision;
    if scope_changed || revision_changed {
        return Err(ApiError::policy_rejected("ticket authorization changed"));
    }
    scope::fence(&tx, &proof).await?;
    super::scope_editor::require_admitted(db, row.object_id).await?;
    tx.commit().await?;
    Ok(TicketContext {
        user_id: row.user_id,
        scope_id: row.scope_id,
        workspace_id: row.workspace_id,
        object_id: row.object_id,
        document_id: row.id,
        client_id: row.client_id,
        authorization_notifications: row.authorization_notifications,
    })
}
#[derive(Default)]
struct Connections {
    entries: HashMap<Uuid, (Uuid, Uuid, Uuid)>,
}
static CONNECTIONS: LazyLock<Mutex<Connections>> = LazyLock::new(|| Mutex::new(Connections::default()));
struct ConnectionPermit(Uuid);
impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        CONNECTIONS.lock().entries.remove(&self.0);
    }
}
fn admit(context: &TicketContext) -> Result<ConnectionPermit, ApiError> {
    let mut state = CONNECTIONS.lock();
    let mut user_count = 0;
    let mut scope_count = 0;
    let mut document_count = 0;
    for (user, scope, document) in state.entries.values() {
        if *user == context.user_id {
            user_count += 1;
        }
        if *scope == context.scope_id {
            scope_count += 1;
        }
        if *document == context.document_id {
            document_count += 1;
        }
    }
    if user_count >= limits::CONNECTIONS_PER_USER_MAX
        || scope_count >= limits::CONNECTIONS_PER_WORKSPACE_MAX
        || document_count >= limits::CONNECTIONS_PER_DOCUMENT_MAX
    {
        return Err(ApiError::limit_exceeded(
            "scope connection budget exhausted",
            "scope_connections",
            Some(json!(limits::CONNECTIONS_PER_WORKSPACE_MAX)),
            None,
            Some(5000),
        ));
    }
    let id = Uuid::new_v4();
    state
        .entries
        .insert(id, (context.user_id, context.scope_id, context.document_id));
    drop(state);
    Ok(ConnectionPermit(id))
}
async fn send(socket: &mut WebSocket, frame: &Frame) -> bool {
    let Ok(encoded) = serde_json::to_string(frame) else {
        return false;
    };
    tokio::time::timeout(Duration::from_secs(2), socket.send(Message::text(encoded)))
        .await
        .is_ok_and(|r| r.is_ok())
}
async fn close(socket: &mut WebSocket, reason: &str) {
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: 4403,
            reason: reason.to_string().into(),
        })))
        .await;
}
const fn reject(
    document: Uuid,
    update: Option<Uuid>,
    code: RejectedCode,
    recoverable: bool,
    write_state: WriteState,
    details: Option<Value>,
) -> Frame {
    Frame::Rejected {
        protocol_version: 1,
        document_id: document,
        update_id: update,
        code,
        recoverable,
        write_state,
        details,
        current_seq: None,
        current_frontier: None,
        audit_event_id: None,
    }
}
fn error_rejection(document: Uuid, update: Uuid, error: &ApiError, client_model: u32) -> Frame {
    let details = match error {
        ApiError::Typed {
            details: Some(details), ..
        } => Some(details.clone()),
        _ => None,
    };
    let code = match error.kind() {
        crate::error::ApiErrorKind::UnsupportedProtocol => RejectedCode::UnsupportedProtocol,
        crate::error::ApiErrorKind::InvalidUpdate => RejectedCode::InvalidUpdate,
        crate::error::ApiErrorKind::LimitExceeded => RejectedCode::LimitExceeded,
        crate::error::ApiErrorKind::FeatureDisabled => RejectedCode::FeatureDisabled,
        crate::error::ApiErrorKind::NotFound | crate::error::ApiErrorKind::PolicyRejected => {
            RejectedCode::PolicyRejected
        }
        _ => RejectedCode::ServerRejected,
    };
    let details = if code == RejectedCode::UnsupportedProtocol {
        details.or_else(|| {
            Some(
                json!({"reason":"editor_upgrade_required","minimum_editor_model":5,"client_editor_model":client_model}),
            )
        })
    } else {
        details
    };
    reject(document, Some(update), code, false, WriteState::NotApplied, details)
}

pub async fn run(mut socket: WebSocket, state: AppState, context: TicketContext) {
    if let Some(workspace) = context.workspace_id {
        let consumed = collab::ticket::ConsumedTicket {
            user_id: context.user_id,
            workspace_id: workspace,
            document_id: context.document_id,
            client_id: context.client_id,
        };
        if context.authorization_notifications {
            collab::session::run_with_authorization_metadata(socket, state, consumed).await;
        } else {
            collab::session::run(socket, state, consumed).await;
        }
        return;
    }
    let Ok(_permit) = admit(&context) else {
        close(&mut socket, "connection budget exhausted").await;
        return;
    };
    let Some(hello) = read_handshake_frame(&mut socket, context.document_id, HandshakePhase::Hello).await else {
        return;
    };
    let Frame::Hello {
        protocol_version: 1,
        capabilities,
        ..
    } = hello
    else {
        close(&mut socket, "expected protocol hello").await;
        return;
    };
    if capabilities
        .iter()
        .any(|c| c != "presence" && c != "rich_page_v4" && c != "rich_page_v5")
    {
        close(&mut socket, "unsupported required capability").await;
        return;
    }
    let editor_model = if capabilities.iter().any(|c| c == "rich_page_v5") {
        5
    } else if capabilities.iter().any(|c| c == "rich_page_v4") {
        4
    } else {
        1
    };
    if super::scope_editor::require_model(&state.db, context.object_id, editor_model)
        .await
        .is_err()
    {
        close(&mut socket, "editor_upgrade_required").await;
        return;
    }
    let presence_hub = if capabilities.iter().any(|c| c == "presence") {
        super::scope_presence::start(&state.db)
            .await
            .ok()
            .filter(|hub| hub.ready())
    } else {
        None
    };
    let mut presence_rx = presence_hub.as_ref().map(|hub| hub.subscribe());
    let session_id = Uuid::new_v4();
    if !send(
        &mut socket,
        &Frame::Hello {
            protocol_version: 1,
            capabilities: if presence_hub.as_ref().is_some_and(|hub| hub.ready()) {
                vec!["presence".into(), "rich_page_v4".into(), "rich_page_v5".into()]
            } else {
                vec!["rich_page_v4".into(), "rich_page_v5".into()]
            },
            client_id: "server".to_string(),
            session_id,
        },
    )
    .await
    {
        return;
    }
    let Some(open) = read_handshake_frame(&mut socket, context.document_id, HandshakePhase::Open).await else {
        return;
    };
    let Frame::Open {
        protocol_version: 1,
        document_id,
        ..
    } = open
    else {
        close(&mut socket, "expected open").await;
        return;
    };
    if document_id != context.document_id {
        close(&mut socket, "document identity mismatch").await;
        return;
    }
    let Ok(boot) = scope_write::bootstrap_model(&state.db, context.object_id, context.user_id, editor_model).await
    else {
        close(&mut socket, "authorization revoked").await;
        return;
    };
    let Ok(mut last_proof) =
        scope::authorize(&state.db, context.object_id, context.user_id, PermissionLevel::View).await
    else {
        close(&mut socket, "authorization revoked").await;
        return;
    };
    let mut authority_sequence = 1u64;
    if context.authorization_notifications {
        let metadata = collab::frame::ResourceAuthorization {
            scope_id: context.scope_id,
            object_id: context.object_id,
            authz_revision: last_proof.revision,
            authority_sequence,
            session_id,
            permitted_actions: collab::frame::permitted_actions(last_proof.level, last_proof.resource_active),
        };
        if !send(
            &mut socket,
            &Frame::Resync {
                protocol_version: 1,
                document_id: context.document_id,
                reason: "authorization_changed".into(),
                minimum_snapshot_seq: None,
                authorization: Some(metadata),
            },
        )
        .await
        {
            return;
        }
    }
    let snapshot = Frame::Snapshot {
        protocol_version: 1,
        document_id: context.document_id,
        snapshot_seq: boot.field("snapshot_seq").as_i64().unwrap_or(0),
        head_seq: boot.field("head_seq").as_i64().unwrap_or(0),
        snapshot: boot.field("snapshot_base64").as_str().unwrap_or("").to_string(),
        tail_updates: serde_json::from_value::<Vec<TailUpdate>>(boot.field("tail_updates").clone()).unwrap_or_default(),
        head_frontier: boot.field("head_frontier").as_str().unwrap_or("").to_string(),
    };
    if collab::compaction::begin_client_view(&state.db, context.document_id, &context.client_id)
        .await
        .is_err()
    {
        close(&mut socket, "document lease unavailable").await;
        return;
    }
    if !send(&mut socket, &snapshot).await {
        return;
    }
    let _presence_lease = presence_hub.as_ref().map(|_| super::scope_presence::Lease {
        db: state.db.clone(),
        event: super::scope_presence::Event {
            object: context.object_id,
            document: context.document_id,
            session: session_id,
            user: context.user_id,
            payload: Value::Null,
            ttl: 0,
        },
    });
    if let Some(hub) = &presence_hub {
        for event in hub.snapshot(context.document_id) {
            if event.session != session_id && event.object == context.object_id {
                if scope::authorize(&state.db, context.object_id, context.user_id, PermissionLevel::View)
                    .await
                    .is_err()
                {
                    close(&mut socket, "authorization revoked").await;
                    return;
                }
                if !send(&mut socket, &event.frame()).await {
                    return;
                }
            }
        }
    }
    let mut sent_seq = boot.field("head_seq").as_i64().unwrap_or(0);
    let mut poll = tokio::time::interval(Duration::from_millis(200));
    let mut window = tokio::time::Instant::now();
    let mut frames = 0u64;
    let mut updates = 0u64;
    let mut last_inbound = tokio::time::Instant::now();
    loop {
        tokio::select! {
                 event=async { match &mut presence_rx { Some(receiver) => receiver.recv().await, None => std::future::pending().await } } => {
                    match event {
                        Ok(event) if event.document == context.document_id && event.object == context.object_id && event.session != session_id => {
                            if scope::authorize(&state.db,context.object_id,context.user_id,PermissionLevel::View).await.is_err() { close(&mut socket,"authorization revoked").await;return; }
                            if !send(&mut socket,&event.frame()).await { return; }
                        },
                        Ok(_) => {},
                        Err(_) => { close(&mut socket,"presence consumer fell behind").await;return; }
                    }
                 }
                 _=poll.tick()=>{
                  if presence_hub.as_ref().is_some_and(|hub|!hub.ready()) { close(&mut socket,"presence transport unavailable").await;return; }
                  if super::scope_editor::require_model(&state.db,context.object_id,editor_model).await.is_err(){close(&mut socket,"editor_upgrade_required").await;return;}
                  if last_inbound.elapsed()>Duration::from_mins(1){close(&mut socket,"heartbeat timeout").await;return;}
                  let Ok(current)=scope::authorize(&state.db,context.object_id,context.user_id,PermissionLevel::View).await else {close(&mut socket,"authorization revoked").await;return;};
                  if context.authorization_notifications&&(current.revision!=last_proof.revision||current.level!=last_proof.level||current.audience_revisions!=last_proof.audience_revisions||current.resource_active!=last_proof.resource_active){
                    authority_sequence=authority_sequence.saturating_add(1);
                    let metadata=collab::frame::ResourceAuthorization{scope_id:context.scope_id,object_id:context.object_id,authz_revision:current.revision,authority_sequence,session_id,permitted_actions:collab::frame::permitted_actions(current.level,current.resource_active)};
                    if !send(&mut socket,&Frame::Resync{protocol_version:1,document_id:context.document_id,reason:"authorization_changed".into(),minimum_snapshot_seq:None,authorization:Some(metadata)}).await{return;}
                  }
                  last_proof=current;

                  let Ok(rows)=PersistedFrame::find_by_statement(Statement::from_sql_and_values(DbBackend::Postgres,"SELECT seq,update_id,bytes,before_frontier,after_frontier,event_id,origin_client_id FROM collab_updates WHERE document_id=$1 AND seq>$2 ORDER BY seq LIMIT 100",vec![context.document_id.into(),sent_seq.into()])).all(&state.db).await else {close(&mut socket,"document unavailable").await;return;};
                  for row in rows{
                   if row.seq!=sent_seq+1{let _=send(&mut socket,&Frame::Resync{protocol_version:1,document_id:context.document_id,reason:"retained update gap".to_string(),minimum_snapshot_seq:None, authorization:None,
        }).await;return;}
                   if scope::authorize(&state.db,context.object_id,context.user_id,PermissionLevel::View).await.is_err(){close(&mut socket,"authorization revoked").await;return;}
                   if !send(&mut socket,&Frame::Update{protocol_version:1,document_id:context.document_id,update_id:row.update_id,base_frontier:BASE64.encode(&row.before_frontier),bytes:BASE64.encode(&row.bytes),idempotency_key:None,origin:row.origin_client_id.unwrap_or_else(||"remote".to_string()),message:None}).await{return;}
                   if !send(&mut socket,&Frame::Accepted{protocol_version:1,document_id:context.document_id,update_id:row.update_id,head_seq:row.seq,head_frontier:BASE64.encode(&row.after_frontier),projection_seq:row.seq,event_id:row.event_id}).await{return;}sent_seq=row.seq;
                  }
                 }
                 inbound=socket.recv()=>{
                  let Some(Ok(message))=inbound else{return;};last_inbound=tokio::time::Instant::now();
                  if window.elapsed()>Duration::from_secs(1){window=tokio::time::Instant::now();frames=0;updates=0;}frames+=1;
                  if frames>limits::FRAME_BURST_MAX{close(&mut socket,"frame rate exceeded").await;return;}
                  let text=match message{Message::Text(text)if text.len()<=limits::WEBSOCKET_FRAME_BYTES_MAX=>text,Message::Ping(bytes)=>{let _=socket.send(Message::Pong(bytes)).await;continue;},Message::Pong(_)=>continue,Message::Close(_)=>return,_=>{close(&mut socket,"invalid frame").await;return;}};
                  let Ok(frame)=serde_json::from_str::<Frame>(&text)else{close(&mut socket,"invalid protocol frame").await;return;};if frame.protocol_version()!=1{close(&mut socket,"unsupported protocol").await;return;}
                  match frame{
                   Frame::Update{document_id,update_id,bytes,idempotency_key,..}=>{
                    updates+=1;if document_id!=context.document_id||updates>limits::UPDATE_BURST_MAX{close(&mut socket,"invalid update admission").await;return;}
                    let result=match BASE64.decode(bytes){Ok(bytes)=>scope_write::accept_model(&state.db,context.object_id,bytes,scope_write::UpdateIntent{user:context.user_id,update:update_id,client:&context.client_id,key:idempotency_key.as_deref(),expected:None,command_body:None},editor_model).await,Err(_)=>Err(ApiError::invalid_update("invalid update bytes"))};
                    match result{Ok(collab::write::AcceptOutcome::Accepted(receipt))=>{if receipt.head_seq>sent_seq {continue;}let frame=Frame::Accepted{protocol_version:1,document_id,update_id,head_seq:receipt.head_seq,head_frontier:BASE64.encode(receipt.head_frontier),projection_seq:receipt.projection_seq,event_id:receipt.event_id};if !send(&mut socket,&frame).await{return;}},Ok(collab::write::AcceptOutcome::Rejected(refusal))=>{let frame=reject(document_id,Some(update_id),refusal.code,refusal.recoverable,refusal.write_state,refusal.details);if !send(&mut socket,&frame).await{return;}},Err(err)=>{if !send(&mut socket,&error_rejection(document_id,update_id,&err,editor_model)).await{return;}}}
                   }
                   Frame::Ping{nonce,..}=>{if !send(&mut socket,&Frame::Pong{protocol_version:1,nonce}).await{return;}},Frame::Ack{document_id,seq,frontier,..}=>{if document_id!=context.document_id{close(&mut socket,"document identity mismatch").await;return;}
                   if let Ok(frontier)=BASE64.decode(frontier){let _=collab::compaction::record_client_ack(&state.db,document_id,&context.client_id,seq,&frontier).await;}},Frame::Presence{document_id,payload,ttl_seconds,..}=>{
                    let ttl=ttl_seconds.unwrap_or(limits::PRESENCE_TTL_SECONDS_DEFAULT);
                    if presence_hub.is_none()||document_id!=context.document_id||ttl==0||ttl>limits::PRESENCE_TTL_SECONDS_MAX||!super::scope_presence::valid_payload(&payload){close(&mut socket,"invalid presence admission").await;return;}
                    if scope::authorize(&state.db,context.object_id,context.user_id,PermissionLevel::View).await.is_err(){close(&mut socket,"authorization revoked").await;return;}
                    let event=super::scope_presence::Event{object:context.object_id,document:context.document_id,session:session_id,user:context.user_id,payload,ttl};
                    if super::scope_presence::publish(&state.db,event).await.is_err(){close(&mut socket,"presence transport unavailable").await;return;}
                   },Frame::Pong{..}=>{},_=>{close(&mut socket,"unexpected frame").await;return;}
                  }
                 }
                }
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    #[test]
    fn an_old_writer_rejection_keeps_identity_and_not_applied_protocol_contract() {
        let document = Uuid::new_v4();
        let update = Uuid::new_v4();
        let frame = error_rejection(document, update, &super::super::scope_editor::model_error(1, 4), 1);
        let Frame::Rejected {
            document_id,
            update_id,
            code,
            write_state,
            details,
            ..
        } = frame
        else {
            panic!("rejection frame")
        };
        assert_eq!(document_id, document);
        assert_eq!(update_id, Some(update));
        assert_eq!(code, RejectedCode::UnsupportedProtocol);
        assert_eq!(write_state, WriteState::NotApplied);
        assert_eq!(
            details,
            Some(json!({"reason":"editor_upgrade_required","minimum_editor_model":4,"client_editor_model":1}))
        );
    }
}
