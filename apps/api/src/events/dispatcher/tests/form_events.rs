//! Form record events through the dispatcher: the four subscribable types leave one
//! `event_dispatch` row each, and what a receiver is sent names the form, the record and the keys of
//! the changed fields, never a value.
#![cfg(test)]

use super::*;
use crate::routes::form::{FormResponse, insert_form_event_with_changes, insert_form_event_with_idempotency};

async fn seed_form(db: &DatabaseConnection, workspace_id: Uuid) -> (FormResponse, Uuid) {
    let owner: Uuid = get_col(
        db,
        "SELECT created_by AS owner FROM workspaces WHERE id = $1",
        vec![workspace_id.into()],
        "owner",
    )
    .await;
    let (project, form, record) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    exec(
        db,
        "INSERT INTO projects (id, workspace_id, key, name, created_by, type_key) \
         VALUES ($1, $2, 'FRM', 'forms', $3, 'code_project')",
        vec![project.into(), workspace_id.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO project_forms (id, workspace_id, project_id, key, name, created_by) \
         VALUES ($1, $2, $3, 'staff', 'Staff', $4)",
        vec![form.into(), workspace_id.into(), project.into(), owner.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO form_records (id, workspace_id, project_id, form_id, title, values, created_by) \
         VALUES ($1, $2, $3, $4, 'Ada earns 9000kr', '{}'::jsonb, $5)",
        vec![
            record.into(),
            workspace_id.into(),
            project.into(),
            form.into(),
            owner.into(),
        ],
    )
    .await;
    let response = FormResponse::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, workspace_id, project_id, key, name, description, icon, color, title_template, schema, \
                detail_layout, schema_version, created_by, archived_at, created_at, updated_at \
           FROM project_forms WHERE id = $1",
        vec![form.into()],
    ))
    .one(db)
    .await
    .expect("form query runs")
    .expect("the form exists");
    (response, record)
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        let read = socket.read(&mut chunk).await.expect("request reads");
        assert!(read > 0, "request ended early");
        bytes.extend_from_slice(&chunk[..read]);
        let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let length = String::from_utf8_lossy(&bytes[..end])
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("numeric length"))
            })
            .unwrap_or(0);
        if bytes.len() >= end + 4 + length {
            return String::from_utf8(bytes).expect("request is UTF-8");
        }
    }
}

#[tokio::test]
async fn the_four_form_record_events_reach_a_subscriber_with_ids_and_field_keys_only() {
    let scratch = scratch_or_skip!("form-record-events");
    let workspace_id = seed_workspace(&scratch.db).await;
    let (form, record) = seed_form(&scratch.db, workspace_id).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("loopback listener binds");
    let address = listener.local_addr().expect("listener address");
    let events = [
        "form.record.created",
        "form.record.updated",
        "form.record.archived",
        "form.record.restored",
    ];
    seed_webhook(&scratch.db, workspace_id, &format!("http://{address}/hook"), &events).await;
    let receiver = tokio::spawn(async move {
        let mut requests = Vec::new();
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.expect("delivery connection arrives");
            requests.push(read_request(&mut socket).await);
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("response writes");
        }
        requests
    });

    // The producers' own transaction writes the events, as the form routes do. The source carries
    // a signature audit entry that must not travel.
    let source =
        json!({ "type": "user", "actor_id": Uuid::new_v4(), "signature_media": { "entries": [{ "url": "/secret" }] } });
    let values = json!({ "name": "Ada", "salary": "9000kr" });
    let tx = scratch.db.begin().await.expect("transaction begins");
    insert_form_event_with_idempotency(
        &tx,
        &form,
        Some(record),
        "form.record.created",
        None,
        source.clone(),
        json!({ "record_id": record, "values": values }),
        None,
    )
    .await
    .expect("created event");
    insert_form_event_with_changes(
        &tx,
        &form,
        Some(record),
        "form.record.updated",
        None,
        source.clone(),
        json!({ "record_id": record, "values": values }),
        None,
        Some(vec!["salary".to_string()]),
    )
    .await
    .expect("updated event");
    for event_type in ["form.record.archived", "form.record.restored"] {
        insert_form_event_with_idempotency(
            &tx,
            &form,
            Some(record),
            event_type,
            None,
            source.clone(),
            json!({ "record_id": record, "title": "Ada earns 9000kr" }),
            None,
        )
        .await
        .expect("lifecycle event");
    }
    // An event of another type leaves no dispatch row.
    insert_form_event_with_idempotency(
        &tx,
        &form,
        Some(record),
        "form.record.recalculated",
        None,
        json!({ "type": "system" }),
        json!({ "record_id": record, "values": values }),
        None,
    )
    .await
    .expect("recalculated event");
    tx.commit().await.expect("commit");

    assert_eq!(
        count(
            &scratch.db,
            "SELECT count(*) AS n FROM event_dispatch WHERE workspace_id = $1 AND event_type LIKE 'form.record.%'",
            vec![workspace_id.into()],
        )
        .await,
        4,
        "one dispatch row for each of the four subscribable types and none for any other"
    );
    for _ in 0..4 {
        expand_one(&scratch.db).await.expect("expansion runs");
    }
    let client = reqwest::Client::new();
    for _ in 0..4 {
        let outcome = TEST_ALLOW_PRIVATE_DELIVERY_TARGET
            .scope(true, send_one(&state_for(scratch.db.clone()), &client))
            .await
            .expect("delivery attempt runs");
        assert_eq!(outcome, Some(true));
    }
    let requests = receiver.await.expect("receiver handled four deliveries");

    let mut seen = std::collections::BTreeMap::new();
    for request in &requests {
        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        for secret in ["9000kr", "Ada", "/secret", "signature_media"] {
            assert!(!body.contains(secret), "{secret} reached the receiver: {body}");
        }
        let body: Value = serde_json::from_str(body).expect("body is JSON");
        let event = &body["event"];
        assert_eq!(event["source"]["type"], "user");
        assert!(event["source"].get("signature_media").is_none());
        seen.insert(
            event["event_type"].as_str().expect("type").to_string(),
            event["payload"].clone(),
        );
    }
    let expected = |fields: Value| json!({ "form_id": form.id, "form_key": "staff", "record_id": record, "changed_fields": fields });
    assert_eq!(seen["form.record.created"], expected(json!(["name", "salary"])));
    assert_eq!(seen["form.record.updated"], expected(json!(["salary"])));
    assert_eq!(seen["form.record.archived"], expected(json!([])));
    assert_eq!(seen["form.record.restored"], expected(json!([])));
    scratch.drop_self().await;
}
