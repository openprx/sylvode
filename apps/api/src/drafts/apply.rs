//! The save of an approved draft: the ordinary save path of its kind, in the review's
//! transaction, with the approver as the author and the approver's rights.

use platform::auth::JwtClaims;
use sea_orm::{ConnectionTrait, DatabaseTransaction, DbBackend, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    prepare::{Plan, Prepared},
    store::DraftRow,
};
use crate::{
    error::ApiError,
    forms::{
        native_create::{NativeFormCreateRequest, create_form_in_transaction},
        permissions::form_action_allowed,
    },
    layouts::{access::Principal, access::require_write, apply::save_layout_draft},
    routes::form::{find_form_with_conn, insert_form_event_with_idempotency, insert_schema_version},
};
use platform::app::AppState;

/// Runs the save and returns the draft's `result`.
#[allow(
    clippy::too_many_arguments,
    reason = "the review hands over everything the save needs"
)]
pub(super) async fn save(
    state: &AppState,
    claims: &JwtClaims,
    tx: &DatabaseTransaction,
    row: &DraftRow,
    plan: Plan,
    prepared: &Prepared,
    approver: Uuid,
    role: &str,
) -> Result<Value, ApiError> {
    let source = json!({ "type": "user", "actor_id": approver, "origin": "draft", "draft_id": row.id });
    match plan {
        Plan::UpdateForm { form_id } => {
            if !form_action_allowed(state, form_id, role, "form.design").await? {
                return Err(ApiError::forbidden("form permission denied: form.design"));
            }
            tx.execute(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "UPDATE project_forms \
                 SET schema = $1, schema_version = schema_version + 1, \
                     updated_at = GREATEST(clock_timestamp(), updated_at + interval '1 microsecond') \
                 WHERE id = $2",
                vec![prepared.spec.clone().into(), form_id.into()],
            ))
            .await?;
            let form = find_form_with_conn(tx, form_id).await?;
            insert_form_event_with_idempotency(
                tx,
                &form,
                None,
                "form.updated",
                Some(approver),
                source,
                json!({ "form_id": form.id, "key": form.key, "changed_fields": ["schema"] }),
                None,
            )
            .await?;
            insert_schema_version(tx, &form, Some(approver), "schema from an approved draft").await?;
            Ok(json!({ "form_id": form.id, "schema_version": form.schema_version }))
        }
        Plan::CreateForm { key, name } => {
            let form = create_form_in_transaction(
                tx,
                NativeFormCreateRequest {
                    form_id: None,
                    workspace_id: row.workspace_id,
                    project_id: row.project_id,
                    key,
                    name,
                    description: String::new(),
                    icon: None,
                    color: None,
                    title_template: None,
                    schema: prepared.spec.clone(),
                    detail_layout: json!({}),
                    created_by: Some(approver),
                    source,
                    change_summary: "initial schema from an approved draft".to_owned(),
                },
            )
            .await?;
            Ok(json!({ "form_id": form.id, "schema_version": form.schema_version }))
        }
        Plan::PlaceLayout { scope, slot } => {
            require_write(state, &Principal::new(claims.clone(), None), &scope).await?;
            save_layout_draft(tx, &scope, &slot, &prepared.spec, approver).await
        }
        // The review admits only human workspace owners and admins, who write every site page.
        Plan::SitePage { target } => {
            crate::site::drafts::apply(tx, row.workspace_id, row.project_id, &target, &prepared.spec, approver).await
        }
    }
}
