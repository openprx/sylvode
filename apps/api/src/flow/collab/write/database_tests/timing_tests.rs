//! Timing measurement (not an assertion) of one 50-block REST `semantic_patch` write at a given
//! page size, with the per-step split taken from the write path's `flow write step` debug events.
//!
//! Run in release mode, one size and model per invocation:
//! `P2_BLOCKS=5000 P2_MODEL=1 cargo test -p api --release --lib timing_tests -- --ignored --nocapture`.
//! The isolated apply runs in this process (`in_process_apply`), so the `isolated_apply` step is
//! the import, projection check and snapshot export without the worker's process start and pipe.

#![allow(clippy::cast_precision_loss)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use serde_json::json;
use uuid::Uuid;

use super::{TEST_DATABASE_URL_ENV, command_input, scratch, seed_workspace, state_for};
use crate::flow::collab::write::in_process_apply;
use crate::flow::command::{CreateObjectInput, create_object_with_editor_model, execute_command};

const BATCH_BLOCKS: usize = 50;
const MEASURED_WRITES: usize = 3;

#[derive(Default)]
struct Steps {
    order: Vec<String>,
    elapsed_us: BTreeMap<String, u64>,
}

struct StepCollector {
    steps: Arc<Mutex<Steps>>,
}

struct StepVisitor {
    step: Option<String>,
    elapsed_us: Option<u64>,
}

impl tracing::field::Visit for StepVisitor {
    fn record_debug(&mut self, _field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "step" {
            self.step = Some(value.to_string());
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "elapsed_us" {
            self.elapsed_us = Some(value);
        }
    }
}

impl tracing::Subscriber for StepCollector {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.is_event() && metadata.target().contains("collab::write")
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}

    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut visitor = StepVisitor {
            step: None,
            elapsed_us: None,
        };
        event.record(&mut visitor);
        let (Some(step), Some(us)) = (visitor.step, visitor.elapsed_us) else {
            return;
        };
        let mut steps = self.steps.lock();
        if !steps.order.contains(&step) {
            steps.order.push(step.clone());
        }
        *steps.elapsed_us.entry(step).or_default() += us;
    }

    fn enter(&self, _span: &tracing::span::Id) {}

    fn exit(&self, _span: &tracing::span::Id) {}
}

fn batch(start: usize) -> serde_json::Value {
    let mut operations = Vec::new();
    for offset in 0..BATCH_BLOCKS {
        let id = format!("b{:05}", start + offset);
        operations.push(json!({"op": "create_node", "id": id, "parent": null, "index": 0, "kind": "block"}));
        operations.push(
            json!({"op": "insert_text", "id": id, "index": 0, "text": format!("benchblock {} alpha", start + offset)}),
        );
    }
    json!({ "operations": operations })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "timing measurement: release build, one size per run, prints a table"]
async fn rest_write_timing_at_page_size() {
    let blocks: usize = std::env::var("P2_BLOCKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1000);
    let model: u32 = std::env::var("P2_MODEL").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let Some(scratch) = scratch(&format!("timing-{blocks}-{model}")).await else {
        eprintln!("skipped: {TEST_DATABASE_URL_ENV} is not set");
        return;
    };
    let state = state_for(scratch.db.clone());
    let (workspace_id, owner_id) = seed_workspace(&state).await;
    let created = create_object_with_editor_model(
        &state,
        CreateObjectInput {
            origin: crate::flow::event_origin::CommandOrigin::first_request_from(
                crate::flow::event_origin::EventSurface::Rest,
            ),
            workspace_id,
            actor_id: owner_id,
            actor_is_bot: false,
            object_type: "page".to_string(),
            project_id: None,
            parent_object_id: None,
            title: "Timing page".to_string(),
            idempotency_key: Uuid::new_v4().to_string(),
            message: None,
        },
        model,
    )
    .await
    .expect("page is created");
    let (object_id, document_id) = (created.object.id, created.object.document_id);
    in_process_apply::enable(document_id);

    let steps = Arc::new(Mutex::new(Steps::default()));
    let _ = tracing::subscriber::set_global_default(StepCollector { steps: steps.clone() });

    let total_batches = blocks / BATCH_BLOCKS;
    let mut measured: Vec<(f64, Steps)> = Vec::new();
    for index in 0..total_batches {
        *steps.lock() = Steps::default();
        let started = Instant::now();
        execute_command(
            &state,
            command_input(
                object_id,
                owner_id,
                "semantic_patch",
                batch(index * BATCH_BLOCKS),
                &Uuid::new_v4().to_string(),
            ),
        )
        .await
        .expect("batch is accepted");
        let wall_ms = started.elapsed().as_secs_f64() * 1000.0;
        if index + MEASURED_WRITES >= total_batches {
            let taken = std::mem::take(&mut *steps.lock());
            measured.push((wall_ms, taken));
        }
    }

    eprintln!(
        "TIMING blocks={blocks} model={model} writes_measured={}",
        measured.len()
    );
    for (i, (wall, _)) in measured.iter().enumerate() {
        eprintln!("TIMING write[{i}] wall_ms={wall:.1}");
    }
    if let Some((wall, taken)) = measured.last() {
        eprintln!("TIMING last-write steps (ms), wall_ms={wall:.1}");
        let mut sum = 0.0;
        for name in &taken.order {
            let ms = taken.elapsed_us.get(name.as_str()).copied().unwrap_or(0) as f64 / 1000.0;
            sum += ms;
            eprintln!("TIMING   {name:<26} {ms:>9.2}");
        }
        eprintln!("TIMING   {:<26} {sum:>9.2}", "sum of steps");
    }
    scratch.drop_self().await;
}
