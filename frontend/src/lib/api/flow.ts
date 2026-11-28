// REST-only adapter for the Flow surface (`contracts/rest-api-v1.md`, v0.4 Flow Alpha table).
//
// This module owns HTTP shape only: it never holds an engine doc, a WebSocket, or UI store
// (`contracts/ui-surface-v1.md` "五个 adapter"). `ObjectRepository`/`CommandService` compose it;
// components never import it directly.
//
// v0.4 baseline note: this repo's baseline now routes `POST /flow/objects/{object_id}/commands`
// and `GET /flow/objects/{object_id}/bootstrap` server-side (`apps/api/src/routes/flow.rs`'s
// `post_flow_object_command`/`get_flow_object_bootstrap`, wired in `main.rs`) -- both are exposed
// below as `executeCommand`/`getBootstrap`. Content edits during an open session still go over
// `ObjectSession`/WebSocket `update` frames, not `commands`; `executeCommand` backs the
// server-governed lifecycle actions (`set_title|insert_block|update_block|delete_block|
// move_block|archive|restore`) `CommandService.execute` needs, and `getBootstrap` backs recovery
// (`ObjectRepository.bootstrap`/`replaceWithAccepted`). `GET /flow/objects/{object_id}/diff` is
// routed too (`routes/flow.rs::get_flow_object_diff`); `getDiff` backs the history panel's
// semantic diff.

import { apiClient, type ApiResult, type UploadProgress } from './client';
import { transportFailure } from './transport';
import { bodyBinaryResult } from '../flow/body/bytes';
import type { MentionedPerson, Person } from '../people/display';
import type { BodyAssetKind, BodyAssetLimits, BodyAssetReceipt } from '../flow/body/assets';

export type { UploadProgress };

/** `apps/api/src/flow/command.rs::REGISTERED_OBJECT_TYPES` (`collection` from v0.6; the generic
 * create endpoint rejects `record`). */
export type FlowObjectType = 'page' | 'navigator' | 'collection';

/** Every `object_type` a read can return: the creatable types plus `record`, which only the
 * collection-scoped records endpoint creates (`flow/collections.rs::create_record`). */
export type FlowReadableObjectType = FlowObjectType | 'record';

export interface FlowObjectView {
	id: string;
	scope_id: string;
	workspace_id?: string;
	permitted_actions?: string[];
	supported_export_formats?: FlowObjectExportFormat[];
	export_unavailable_reason?: string;
	history_export_supported?: boolean;
	package_snapshot_history_retained?: boolean;
	editor_model?: 'plain_v1' | 'rich_page_v4' | 'rich_page_v5';
	minimum_editor_model?: 1 | 4 | 5;
	editor_structure_state?: 'ready' | 'initialization_required';
	body_asset_authentication?: 'user_session';
	child_page_authentication?: 'user_session';
	historical_asset_reads_supported?: boolean;
	editor_state?: 'ready' | 'migration_required' | 'unsupported';
	editor_initialization_supported?: boolean;
	record_context_supported?: boolean;
	record_content_writable?: boolean;
	record_archive_allowed?: boolean;
	record_readonly_reason?:
		| 'record_inactive'
		| 'insufficient_access'
		| 'scope_unavailable'
		| 'container_inactive';
	supported_forms_conversion?: boolean;
	forms_conversion_destination_mode?: 'same_workspace' | 'explicit_workspace' | 'unavailable';
	project_id: string | null;
	object_type: FlowReadableObjectType;
	lifecycle_status: string;
	governance_metadata: Record<string, unknown>;
	title: string;
	semantic_content: unknown;
	/** Present only for the explicit Markdown render query. */
	rendered_markdown?: string;
	document_id: string;
	document_seq: number;
	frontier: string;
	projection_seq: number;
	projection_lag: number;
	created_at: string;
	updated_at: string;
	archived_at: string | null;
	parent_id?: string | null;
	appearance?: FlowAppearance;
}

export interface AcceptedChange {
	object: FlowObjectView;
	accepted_seq: number;
	head_frontier: string;
	projection_seq: number;
	semantic_diff: unknown;
	affected_object_ids: string[];
	event_id: string;
	command_result?: unknown;
}

export interface FlowObjectListResponse {
	items: FlowObjectView[];
	next_cursor?: string;
}

export type FlowPageIcon = { kind: 'emoji'; emoji: string } | { kind: 'image'; asset_id: string };

export type FlowPageCover =
	| { kind: 'gallery'; gallery_id: string; position: number }
	| { kind: 'upload'; asset_id: string; position: number };

export interface FlowAppearance {
	icon?: FlowPageIcon | null;
	cover?: FlowPageCover | null;
}

export interface FlowNavigatorNode {
	scope_id?: string;
	mount_id?: string;
	ownership?: 'external' | 'owned';
	mount_status?: 'active' | 'source_archived';
	can_remove?: boolean;
	object_id: string;
	parent_id: string;
	position: string;
	title: string;
	type: FlowReadableObjectType;
	appearance?: FlowAppearance;
}

export interface FlowNavigatorResponse {
	root_object_id: string;
	nodes: FlowNavigatorNode[];
	document_seq: number;
	frontier: string;
}

export interface FlowHistoryEntry {
	seq: number;
	actor: string;
	origin: string;
	message: string | null;
	// Server-observed shape (`apps/api/src/flow/query.rs`) is a structured JSON object (accepted
	// seq/changed block ids/document/object id), not the rendered string `rest-api-v1.md`'s
	// public-type sketch implies -- typed `unknown` here and rendered defensively.
	semantic_summary: unknown;
	created_at: string;
}

export interface FlowHistoryResponse {
	items: FlowHistoryEntry[];
	next_before_seq?: number;
}

export interface CreateFlowObjectInput {
	editor_model?: 4 | 5;
	object_type: FlowObjectType;
	project_id?: string;
	parent_object_id?: string;
	title: string;
	idempotency_key: string;
	message?: string;
	/** v0.6 standalone Collection only (`routes/flow.rs::CreateFlowObjectRequest`; the server
	 * rejects both for any other `object_type`, `flow/command.rs` "initial_fields and
	 * initial_view require object_type=collection"). */
	initial_fields?: FlowCollectionInitialField[];
	initial_view?: FlowCollectionInitialView;
}

/** One `initial_fields[]` entry (`flow/collections.rs::FieldCreatePayload`). The ids are minted by
 * the client and are part of the idempotency body, so a retry must resend the same ones. */
export interface FlowCollectionInitialField {
	field_id: string;
	label: string;
	field_type: 'text' | 'number' | 'boolean' | 'date' | 'select' | 'multi_select' | 'relation';
	config: Record<string, unknown>;
	index: number;
}

/** `initial_view` (`flow/collections.rs::ViewCreatePayload`; `view_type` is `table|board`). */
export interface FlowCollectionInitialView {
	view_id: string;
	name: string;
	view_type: 'table' | 'board';
	config: Record<string, unknown>;
	index: number;
}

/** `GET /workspaces/{id}/flow/search` query (`routes/flow.rs::FlowSearchQuery`). Exactly one of
 * `project_id` / `unprojected` / `all_visible` (`flow/search.rs::validate`); `object_type` is
 * `page|navigator` only. */
export interface FlowSearchQuery {
	q: string;
	project_id?: string;
	unprojected?: boolean;
	all_visible?: boolean;
	object_type?: 'page' | 'navigator';
	freshness?: 'allow_stale' | 'require_current';
	cursor?: string;
	limit?: number;
}

/** `flow/model.rs::FlowSearchHit`. `snippets` keys are `title` / `plain_text`; values are
 * HTML-escaped text with `<mark>`/`</mark>` around matches (`flow/search.rs::fetch_search_batch`). */
export interface FlowSearchHit {
	object: {
		id: string;
		object_type: string;
		title: string;
		lifecycle_status: string;
		project_id?: string;
	};
	matched_fields: string[];
	snippets: Record<string, string>;
	indexed_seq: number;
	head_seq: number;
	projection_lag: number;
	stale: boolean;
}

/** `flow/model.rs::FlowSearchResponse` (no total by design). */
export interface FlowSearchResponse {
	items: FlowSearchHit[];
	next_cursor?: string;
	index_frontier: { indexed_seq: number; head_seq: number; lag: number; stale: boolean };
}

export interface ListFlowObjectsQuery {
	project_id?: string;
	unprojected?: boolean;
	object_type?: FlowObjectType;
	parent_id?: string;
	q?: string;
	cursor?: string;
	limit?: number;
	include_archived?: boolean;
}

export interface CreateTicketInput {
	editor_model?: 4 | 5;
	authorization_notifications?: boolean;
	scope_id?: string;
	object_id?: string;
	workspace_id?: string;
	document_id: string;
	client_id: string;
	origin: string;
}

export interface CollabTicket {
	authorization_notifications?: boolean;
	scope_id?: string;
	object_id?: string;
	document_id?: string;
	authz_revision?: number;
	websocket_path?: string;
	ticket: string;
	expires_at: string;
	websocket_url?: string;
	limits?: unknown;
}

export interface CollabDiagnostics {
	scope_id: string;
	object_id: string;
	document_id: string;
	engine: string;
	format_version: string;
	snapshot_seq: number;
	head_seq: number;
	frontier: string;
	update_count: number;
	byte_size: number;
	last_compacted_at: string | null;
	projection_seq: number;
	integrity_state: string;
}

/** `flow_workspace_settings.default_member_level`'s closed set (`rest-api-v1.md` v0.4 table). */
export const FLOW_MEMBER_LEVELS = ['full_access', 'edit', 'comment', 'view'] as const;
export type FlowMemberLevel = (typeof FLOW_MEMBER_LEVELS)[number];

export interface FlowFeatureFlags {
	flow_enabled: boolean;
	default_member_level: FlowMemberLevel;
	/** Whether Flow pages may be converted into forms and form records in this workspace. */
	bridge_enabled: boolean;
	/** Read-only. Advanced server-side by every authorization-affecting transition. */
	authz_epoch: number;
	/** `null` while the workspace has no `flow_workspace_settings` row yet. */
	updated_at: string | null;
	updated_by: string | null;
}

/** `PUT /workspaces/{workspace_id}/features/flow` request. The fields are optional but the
 * request must carry at least one of them (`rest-api-v1.md`); callers build it through
 * `FlowSettingsController`, which refuses an empty change before any request is made. */
export interface SetFlowFeatureInput {
	enabled?: boolean;
	default_member_level?: FlowMemberLevel;
	bridge_enabled?: boolean;
	idempotency_key: string;
}

/** `PUT` response: the `GET` shape plus `event_id` (`null` when nothing observable changed). */
export interface FlowFeatureUpdate extends FlowFeatureFlags {
	event_id: string | null;
}

export interface BridgePermissionState {
	access: 'read_only' | 'controlled';
	configuration: 'explicit' | 'unconfigured';
	actions: string[];
	field_read_limited: boolean;
	field_write_limited: boolean;
	record_limited: boolean;
}

/** One `GET /flow/objects/{id}/references` item (`flow/bridge.rs::BridgeReferenceView`, tagged
 * by `visibility`). An `unavailable` item carries nothing else: the target was archived or is no
 * longer readable, and the server does not say which. */
export type FlowReferenceItem =
	| {
			visibility: 'available';
			reference_id: string;
			source_object_id: string;
			target_type: 'form' | 'form_record';
			target_id: string;
			title: string;
			status: string;
			summary: unknown;
			display: Record<string, unknown>;
			permission_state: BridgePermissionState;
	  }
	| { visibility: 'unavailable' };

export interface FlowReferenceList {
	items: FlowReferenceItem[];
}

export interface FlowReferenceReceipt {
	reference_id: string;
	source_object_id: string;
	target_type: 'form' | 'form_record';
	target_id: string;
	lineage_id: string | null;
	permission_state: BridgePermissionState;
}

/** `POST /flow/conversions/preview` body (`rest-api-v1.md` v0.7 table). `mapping` is free-form on
 * the wire; its real shape is per `target_type` (`apps/api/src/flow/bridge.rs::preview_conversion`
 * and `execute_conversion`): `form_record` reads `target_form_id` (required), `title` (optional,
 * defaults to the Page title) and `values` (constant field values keyed by form field key). */
export interface FlowConversionPreviewInput {
	destination_workspace_id?: string;
	destination_project_id?: string;
	source_object_id: string;
	source_frontier: string;
	target_type: 'form' | 'form_record';
	mapping: Record<string, unknown>;
	idempotency_key: string;
}

/** `POST /flow/conversions` body. `preview_id`, `source_frontier` and `target_schema_version`
 * must be copied from the preview response. */
export interface FlowConversionCommitInput {
	preview_id: string;
	source_frontier: string;
	target_schema_version: number;
	idempotency_key: string;
	confirm: true;
}

export interface FlowConversionPreview {
	preview_id: string;
	expires_at: string;
	source_frontier: string;
	target_schema_version: number;
	mapping: Record<string, unknown>;
	warnings: string[];
	permission_decision: BridgePermissionState;
	estimated_objects: number;
}

export interface FlowConversionJob {
	job_id: string;
	status: 'started' | 'completed' | 'failed';
	source_object_id: string;
	/** What the conversion creates (`rest-api-v1.md` v0.7 job row; B4). */
	target_type: 'form' | 'form_record';
	source_frontier: string;
	target_schema_version: number;
	lineage_id: string | null;
	created_target_ids: string[];
	warnings: unknown[];
	error: string | null;
	/** `"stale_started"` only on a `failed` job the server reaped after it stayed `started` past
	 * the threshold; absent otherwise (`rest-api-v1.md` v0.7, 2026-10-10). */
	error_reason?: 'stale_started' | null;
}

export type FlowPackageConflictPolicy = 'reject_existing' | 'reuse_import_lineage';
export type FlowPackageExternalReferencePolicy = 'reject' | 'detach';

export interface FlowPackageArtifactReceipt {
	artifact_id: string;
	package_sha256: string;
	size: number;
	expires_at: string;
}

/** `manifest_summary.projects[]` row: a source project the package's objects belong to. `title`
 * is `null` for packages exported without the optional manifest `projects` field. */
export interface FlowPackageManifestProject {
	source_project_id: string;
	title: string | null;
}

/** `imports/preview` `manifest_summary` (`rest-api-v1.md` v0.8, 2026-10-10;
 * `apps/api/src/flow/package_import.rs::ManifestSummary`). */
export interface FlowPackageManifestSummary {
	/** Verified archive provenance only. V1 has genuine workspace_id, not an inferred scope. */
	source?: {
		scope_id?: string;
		workspace_id?: string;
		scope: 'object' | 'workspace' | 'scope';
		root_object_ids: string[];
	};
	/** included controls separate update/event members, never the snapshot's internal history. */
	history?: { included: boolean; snapshot_history: 'retained' };
	projects: FlowPackageManifestProject[];
	object_count: number;
	document_count: number;
	package_schema: string;
	engine: { name: string; crate_version: string; wire_format_version: number };
}

/** One structured preview conflict (`package_import.rs::ImportConflict`, tagged by `kind`). */
export type FlowPackageConflictDetail =
	| { kind: 'lineage_exists'; source_object_id: string; existing_target_id: string }
	| { kind: 'project_unmapped'; source_project_id: string; title: string | null };

export interface FlowPackageImportPreview {
	preview_id: string;
	package_id: string;
	package_sha256: string;
	mapping_hash: string;
	manifest_summary: FlowPackageManifestSummary;
	mapping: Record<string, unknown>;
	/** Bare subject ids, kept for compatibility; `conflict_details` carries the kinds. */
	conflicts: string[];
	conflict_details: FlowPackageConflictDetail[];
	warnings: string[];
	estimated_changes: Record<string, number>;
	expires_at: string;
}

/** `POST /workspaces/{workspace_id}/flow/exports` body (`rest-api-v1.md` v0.8 table). The
 * workspace scope only accepts `format:"package"` at the accepted head (no `at_seq`). */
export type FlowPackageObjectType = 'page' | 'navigator';

export interface CreateWorkspaceExportInput {
	/** Omitted preserves all-owned legacy selection; new UI sends an explicit supported-kind filter. */
	object_types?: FlowPackageObjectType[];
	format: 'package';
	include_history: boolean;
	project_id?: string;
	idempotency_key: string;
}

/** Export job receipt. `POST .../exports` returns it without `download_url`; `GET
 * /flow/exports/{job_id}` adds `download_url` only for `completed`
 * (`apps/api/src/routes/flow.rs::get_flow_export`). A render that fails inside the request is
 * still a success envelope with `status:"failed"` and `error` set to a stable code, with an empty
 * `checksum` and `size: 0` (`rest-api-v1.md` v0.8, 2026-10-10). */
export interface FlowExportJob {
	/** Persisted server selection/omission warning keys. Missing/empty older metadata does not prove a complete scope. */
	warnings?: string[];
	scope_id?: string;
	job_id: string;
	status: string;
	format: string;
	workspace_id?: string | null;
	object_id?: string | null;
	package_schema?: string;
	checksum: string | null;
	size: number | null;
	expires_at: string | null;
	download_url?: string;
	error?: string | null;
}

export interface FlowPackageImportJobReceipt {
	job_id: string;
	import_id: string;
	status: string;
}

export interface FlowPackageImportReport {
	import_id: string;
	package_id: string;
	package_sha256: string;
	source_scope_id?: string | null;
	target_scope_id: string;
	/** Historical provenance and legacy integration only; never fabricated for personal jobs. */
	source_workspace_id?: string | null;
	target_workspace_id?: string | null;
	status: string;
	mapping_hash: string;
	conflict_policy: FlowPackageConflictPolicy;
	counts: Record<string, number>;
	object_mapping: Record<string, string>;
	document_mapping: Record<string, string>;
	detached_references: string[];
	warnings: string[];
	started_at: string;
	finished_at: string;
	actor: string;
	audit_event_id: string;
}

/** `GET /admin/workspaces/{workspace_id}/flow/health` (`rest-api-v1.md` v0.8 table;
 * `apps/api/src/flow/operations.rs::workspace_health`). Ages are seconds (`null` when nothing is
 * pending/failed). `delivery_cancelled` is a normal cancellation (`subscriber_gone`) and is NOT
 * part of `dead_letter`. The server emits `status` as `healthy` or `degraded`. */
export interface FlowAdminHealth {
	status: string;
	connections: number;
	accept_rate: number;
	reject_rate: number;
	queue_depth: number;
	oldest_job_age: number | null;
	storage_bytes: number;
	dead_letter: {
		dispatch_failed: number;
		delivery_failed: number;
		oldest_failed_age: number | null;
	};
	delivery_cancelled: number;
}

/** One per-document row of an admin lag group (`rest-api-v1.md` v0.8, frozen 2026-10-10):
 * `{object_id,document_id,head_seq,<projection_seq|indexed_seq|fanout_seq>,lag}`. */
export interface FlowAdminLagItem {
	object_id: string;
	document_id: string;
	head_seq: number;
	projection_seq?: number;
	indexed_seq?: number;
	fanout_seq?: number;
	lag: number;
}

export interface FlowAdminLagGroup {
	/** Workspace-wide aggregates. */
	max: number;
	p95: number;
	/** The current page of documents (same documents in all three groups). */
	items: FlowAdminLagItem[];
}

/** `GET /admin/workspaces/{workspace_id}/flow/lag`. The server pages the per-document items
 * (`cursor?`, `limit<=100`, default 50; the last page omits `next_cursor`); the operations panel
 * pages through them. */
export interface FlowAdminLag {
	projection: FlowAdminLagGroup;
	search: FlowAdminLagGroup;
	fanout: FlowAdminLagGroup;
	next_cursor?: string | null;
}

/** A healthy `documents[]` row of `GET .../flow/integrity?scope=documents`
 * (`apps/api/src/flow/collab/integrity.rs::DocumentFingerprint` plus `status:"ok"`,
 * `checked_at`). */
export interface FlowDocumentFingerprint {
	workspace_id: string;
	object_id: string;
	document_id: string;
	status?: 'ok';
	checked_at?: string;
	head_seq: number;
	head_frontier: string;
	semantic_hash: string;
	projection_seq: number;
}

/** `integrity.failure` (`rest-api-v1.md` v0.8, frozen 2026-10-10). */
export type FlowIntegrityFailure =
	| 'storage_inconsistent'
	| 'projection_missing'
	| 'limit_exceeded'
	| 'document_missing'
	| 'replay_failed';

/** A failed `documents[]` row: the document is listed with why it failed. */
export interface FlowIntegrityFailedDocument {
	workspace_id: string;
	object_id: string;
	document_id: string;
	status: 'failed';
	checked_at: string;
	failure: FlowIntegrityFailure | string;
	error_code: string;
}

export interface FlowAdminIntegrity {
	status: string;
	checked_at: string;
	/** Always the whole workspace; only `documents[]` is paged. */
	counts: { total: number; checked: number; healthy: number; failed: number };
	documents?: Array<FlowDocumentFingerprint | FlowIntegrityFailedDocument> | null;
	next_cursor?: string | null;
}

/** One `OperationReceipt.changes[]` entry (`rest-api-v1.md` v0.8 `OperationReceipt`, frozen
 * 2026-10-10). Compact: before is `{snapshot_seq,snapshot_checksum}`, or on a dry run
 * `{snapshot_seq,tail_updates,tail_bytes}`; rebuild: before/after `{hash}`. */
export type FlowOperationChange =
	| {
			kind: 'compaction';
			document_id: string;
			changed: boolean;
			before: Record<string, unknown>;
			after: Record<string, unknown>;
			deleted_updates: number;
	  }
	| {
			kind: 'projection_rebuild';
			object_id: string;
			changed: boolean;
			before: { hash: string | null };
			after: { hash: string | null };
	  };

/** `OperationReceipt` (`apps/api/src/flow/operations.rs::OperationReceipt`). The contract fields
 * (`mode`, `scope`, `changes`, `warnings`, `started_at`, `finished_at`, `audit_event_id`) sit next
 * to the earlier `operation` / `dry_run` / `result` fields, which the server keeps. */
export interface FlowOperationReceipt {
	operation_id: string;
	operation: string;
	status: string;
	dry_run: boolean;
	workspace_id: string;
	object_id: string;
	document_id: string;
	expected_head_seq: number;
	result: Record<string, unknown>;
	mode: 'dry_run' | 'execute';
	scope: {
		kind: 'document' | 'object';
		workspace_id: string;
		object_id: string;
		document_id: string;
	};
	observed_head_seq?: number | null;
	changes: FlowOperationChange[];
	/** Stable codes, never free text. */
	warnings: string[];
	started_at: string;
	finished_at?: string | null;
	/** Never empty (`rest-api-v1.md`): execute points at the business event, a dry run or verify at
	 * its audit-only previewed event. */
	audit_event_id?: string | null;
}

export interface VerifyDocumentInput {
	dry_run: true;
	deep: boolean;
	expected_head_seq?: number;
	idempotency_key: string;
}

export interface CompactDocumentInput {
	dry_run: boolean;
	expected_head_seq: number;
	retain_after_seq?: number;
	confirm_document_id?: string;
	idempotency_key: string;
}

export interface RebuildProjectionInput {
	dry_run: boolean;
	expected_head_seq: number;
	confirm_object_id?: string;
	idempotency_key: string;
}

export type FlowReplayMode = 'rebuild' | 'requeue_failed';

/** `POST /admin/workspaces/{workspace_id}/flow/deliveries/replay`. The server requires
 * `confirm: true` on every request, dry-run included (`routes/flow.rs::post_flow_delivery_replay`). */
export interface ReplayDeliveriesInput {
	mode: FlowReplayMode;
	event_type?: string;
	subscriber_kind?: string;
	subscriber_id?: string;
	from: string;
	to: string;
	dry_run: boolean;
	confirm: true;
	idempotency_key: string;
}

export interface FlowReplayWindow {
	from: string;
	to: string;
}

/** `ReplayResult` is untagged on the wire: the mode is only recoverable from the request. */
export type FlowReplayResponse =
	| {
			replayed: number;
			skipped_already_delivered: number;
			rebuilt_delivery_ids: string[];
			window: FlowReplayWindow;
	  }
	| {
			requeued: number;
			skipped_not_failed: number;
			requeued_delivery_ids: string[];
			window: FlowReplayWindow;
	  };

/** `GET /workspaces/{workspace_id}/flow/projection-lag` (`apps/api/src/flow/model.rs`). */
export interface FlowProjectionLagItem {
	object_id: string;
	head_seq: number;
	projection_seq: number;
	lag: number;
}

export interface FlowProjectionLag {
	max_lag: number;
	p95_lag: number;
	items: FlowProjectionLagItem[];
	next_cursor?: string;
}

/** v0.4 command types (`rest-api-v1.md`'s `POST .../commands` row) plus the v0.6 Collection
 * commands (`flow/collections.rs::CollectionCommandType::parse`; `record_create` and
 * `record_query` are not sent here: records are created through the records endpoint and the
 * command form of a query is refused). */
export type FlowCommandType =
	| 'initialize_editor_doc'
	| 'insert_body_asset'
	| 'replace_body_asset'
	| 'link_child_page'
	| 'link'
	| 'unlink'
	| 'set_title'
	| 'insert_block'
	| 'update_block'
	| 'delete_block'
	| 'move_block'
	| 'archive'
	| 'restore'
	| 'move_object'
	| FlowCollectionCommandType;

export type FlowCollectionCommandType =
	| 'field_create'
	| 'field_update'
	| 'field_archive'
	| 'field_reorder'
	| 'view_create'
	| 'view_update'
	| 'view_reorder'
	| 'record_patch'
	| 'record_archive'
	| 'create_collection_embed';

/** `create_collection_embed` payload (`flow/collections.rs::CreateCollectionEmbedPayload`): one
 * Page command that creates the Collection, its document and projection, the Page's embed block
 * and the `embeds` relation in a single transaction. The field and view ids are client-minted and
 * part of the replay body, so a retry must resend them unchanged. */
export interface CreateCollectionEmbedPayload {
	title: string;
	initial_fields?: FlowCollectionInitialField[];
	initial_view?: FlowCollectionInitialView;
}

/** `AcceptedChange.command_result` of `create_collection_embed` as the server writes it
 * (`flow/collections.rs::create_collection_embed`, the `command_result` object). */
export interface CollectionEmbedCommandResult {
	collection_id: string;
	collection_document_id: string;
	block_id: string;
}

/** The seven first-release field types (`flow/collections.rs::validate_field_type`). */
export type FlowCollectionFieldType =
	| 'text'
	| 'number'
	| 'boolean'
	| 'date'
	| 'select'
	| 'multi_select'
	| 'relation';

/** `flow/collections.rs::CollectionFieldView` (the type is serialized as `type`). `config` is a
 * free-form object; the server only reads `config.restricted` (`validate_field_config`). */
export interface FlowCollectionFieldView {
	field_id: string;
	label: string;
	type: string;
	config: Record<string, unknown>;
	position: number;
	archived: boolean;
	document_seq: number;
}

/** `flow/collections.rs::CollectionViewView` (`type` is `table|board`). */
export interface FlowCollectionViewView {
	view_id: string;
	name: string;
	type: string;
	config: Record<string, unknown>;
	position: number;
	document_seq: number;
}

/** `GET /flow/collections/{id}` and `GET /flow/objects/{id}/schema` for a Collection
 * (`flow/collections.rs::CollectionDescription`). Restricted fields are already removed when
 * `field_secrecy_enabled` (`describe_collection`). */
export interface FlowCollectionDescription {
	object: FlowObjectView;
	fields: FlowCollectionFieldView[];
	views: FlowCollectionViewView[];
	record_count: number;
	schema_seq: number;
	projection_seq: number;
	client_crdt_enabled: boolean;
	field_secrecy_enabled: boolean;
}

/** `flow/collections.rs::RecordFilter`: one condition. `op` is `eq|ne|lt|lte|gt|gte` for scalar
 * types and `eq|ne|contains` for `multi_select`/`relation`; `value` must be the field's wire type. */
export interface FlowRecordFilter {
	field_id: string;
	op: string;
	value: unknown;
}

/** `flow/collections.rs::RecordSort` (`direction` defaults to `asc`). */
export interface FlowRecordSort {
	field_id: string;
	direction: 'asc' | 'desc';
}

/** `GET /flow/collections/{id}/records` query (`routes/flow.rs::GetCollectionRecordsQuery`):
 * `filter`/`sort` are JSON-encoded, `field_ids` comma-separated, `limit` 1..=100 (default 50).
 * `view_id` applies that view's saved filter/sort/group; an explicit `filter`/`sort`/`group`
 * overrides the saved value for that part only. */
export interface FlowCollectionRecordsQuery {
	view_id?: string;
	filter?: FlowRecordFilter;
	sort?: FlowRecordSort;
	group?: string;
	cursor?: string;
	limit?: number;
	field_ids?: string[];
}

/** `POST /flow/collections/{id}/query` body (`flow/collections.rs::RecordQueryPayload`). */
export interface FlowCollectionQueryInput {
	filter?: FlowRecordFilter | null;
	sort?: FlowRecordSort | null;
	group?: string | null;
	cursor?: string | null;
	limit?: number;
	field_ids?: string[];
}

export interface FlowRecordItem {
	record: FlowObjectView;
	values_by_field_id: Record<string, unknown>;
}

/** `flow/collections.rs::RecordQueryResponse`. */
export interface FlowCollectionRecordsResponse {
	fields: FlowCollectionFieldView[];
	items: FlowRecordItem[];
	next_cursor?: string;
	projection_seq: number;
	schema_seq: number;
}

/** Bounded Record-only context; no parent identity/revision/views/count/siblings. */
export interface FlowRecordFieldView {
	field_id: string;
	label: string;
	type: string;
	config: Record<string, unknown>;
	position: number;
	archived: boolean;
}
export interface FlowRecordContext {
	record: FlowObjectView;
	fields: FlowRecordFieldView[];
	values_by_field_id: Record<string, unknown>;
	field_secrecy_enabled: boolean;
	schema_revision: string;
}

/** `POST /flow/collections/{id}/records` (`routes/flow.rs::CreateCollectionRecordRequest`). */
export interface CreateCollectionRecordInput {
	values_by_field_id: Record<string, unknown>;
	body?: string;
	idempotency_key: string;
	message?: string;
}

export interface FlowComment {
	anchor_status?: 'bound' | 'orphaned' | null;
	id: string;
	object_id: string;
	block_id?: string | null;
	parent_id?: string | null;
	author_id?: string | null;
	/** The author as a person to show; null when no author remains. */
	author?: Person | null;
	body: string;
	/** The people the saved comment mentions: render and highlight from this list. */
	mentions: MentionedPerson[];
	created_at: string;
	updated_at: string;
	resolved_at?: string | null;
	/** Who resolved the comment, as a person to show; null while it is open. */
	resolved_by?: Person | null;
}

export interface FlowCommentList {
	items: FlowComment[];
	truncated: boolean;
}

export interface CreateFlowCommentInput {
	body: string;
	block_id?: string | null;
	parent_id?: string | null;
	mentions?: string[];
}

export interface FlowShareLink {
	id: string;
	token_prefix: string;
	expires_at?: string | null;
	revoked_at?: string | null;
	created_at: string;
	created_by?: string | null;
}

export interface FlowShareLinkList {
	items: FlowShareLink[];
}

export interface CreatedFlowShareLink {
	link: FlowShareLink;
	token: string;
}

export interface FlowPublicBlock {
	asset_id?: string;
	alt?: string;
	caption?: string;
	name?: string;
	mime?: string;
	child_page_id?: string;
	inline_spans?: import('../flow/inline-runs').PageInlineSpan[];
	checked?: boolean;
	order?: number;
	collapsed?: boolean;
	kind: string;
	text: string;
	level?: number | null;
	indent: number;
}

export interface FlowChildReservation {
	state: 'pending' | 'expired' | 'cancelled' | 'finalized';
	reservation_id: string;
	child_page_id: string;
	source_object_id: string;
	title: string;
	expires_at: string;
}

export interface FlowPublicShare {
	object_id: string;
	title: string;
	plain_text: string;
	blocks: FlowPublicBlock[];
	appearance?: FlowAppearance;
	expires_at?: string | null;
}

export interface FlowMentionCandidate {
	user_id: string;
	/** Empty for a user without a name: show the neutral text, never the id. */
	name: string;
	kind: 'member' | 'guest' | string;
}

export interface FlowTrashItem {
	id: string;
	parent_id?: string | null;
	object_type: string;
	title: string;
	archived_at: string;
	appearance?: FlowAppearance;
}

export interface FlowTrashPage {
	items: FlowTrashItem[];
	next_cursor?: string;
}

export interface FlowRestoreResult {
	object: FlowObjectView;
	target_seq: number;
	head_seq: number;
	changed: boolean;
}

export interface FlowCommandInput {
	type: FlowCommandType;
	payload?: unknown;
}

export interface ExecuteFlowCommandInput {
	command: FlowCommandInput;
	expected_frontier?: string;
	idempotency_key: string;
	message?: string;
}

export interface TailUpdateEntry {
	seq: number;
	update_id: string;
	/** Base64 of the raw CRDT update bytes. */
	bytes: string;
	before_frontier: string;
	after_frontier: string;
}

/** `GET .../bootstrap` response (`apps/api/src/flow/model.rs::Bootstrap`). */
export interface FlowBootstrap {
	scope_id?: string;
	object_id: string;
	document_id: string;
	engine: string;
	format_version: string;
	snapshot_seq: number;
	head_seq: number;
	/** Base64 of the full document snapshot bytes. */
	snapshot_base64: string;
	tail_updates: TailUpdateEntry[];
	head_frontier: string;
	/** Raw `sylvode.flow.limits.v1` wire object (snake_case fields + `version`). Deliberately
	 * `unknown`: it is untrusted until `flow/limits.ts::negotiateFlowLimitsVersion` proves the
	 * `version` is one this client implements and the payload is complete. */
	limits: unknown;
	websocket_path: string;
}

/** `GET .../diff` response (`apps/api/src/flow/model.rs::ObjectDiffResponse`). `semantic_diff` is
 * `{title:{before,after}|null,nodes:{added,removed,changed}}` (`flow/query.rs::get_object_diff`). */
export interface FlowDiffResponse {
	object_id: string;
	from_seq: number;
	to_seq: number;
	from_frontier: string;
	to_frontier: string;
	semantic_diff: unknown;
	rendered?: string;
}

/** `ADR-0012` grant levels (`flow_object_grants.level`). `none` only ever appears as an
 * effective/before/after level in a summary, never as a stored grant. */
export type FlowGrantLevel = 'full_access' | 'edit' | 'comment' | 'view';
export type FlowEffectiveLevel = FlowGrantLevel | 'none';
/** `flow_object_grants.principal_kind`'s registered values (`apps/api/src/flow/grants.rs`
 * `PRINCIPAL_KINDS`). */
export type FlowPrincipalKind = 'user' | 'bot';

export interface FlowGrantInput {
	principal_kind: FlowPrincipalKind;
	principal_id: string;
	level: FlowGrantLevel;
}

/** One explicit grant on the object (`grants.rs::GrantItem`). */
export interface FlowGrantItem extends FlowGrantInput {
	granted_by: string | null;
	created_at: string;
}

/** One grant contributed by an ancestor (`grants.rs::InheritedGrantItem`). */
export interface FlowInheritedGrantItem extends FlowGrantInput {
	object_id: string;
}

/** `GET /flow/objects/{object_id}/grants` (`grants.rs::GrantsView`). `items`/`inherited` are
 * always empty arrays unless the caller holds `full_access` on the object. */
export interface FlowGrantsView {
	items: FlowGrantItem[];
	inherit_from_parent: boolean;
	inherited: FlowInheritedGrantItem[];
	effective_level: FlowEffectiveLevel;
}

/** `grants.rs::PermissionChanges`, identical for a dry run and a real request. */
export interface FlowPermissionChanges {
	caller: {
		before_level: FlowEffectiveLevel;
		after_level: FlowEffectiveLevel;
		loses_full_access: boolean;
	};
	affected: Array<{
		principal_kind: FlowPrincipalKind;
		principal_id: string;
		before_level: FlowEffectiveLevel;
		after_level: FlowEffectiveLevel;
	}>;
}

/** `PUT /flow/objects/{object_id}/grants` body. `grants` is the complete replacement roster. */
export interface SetFlowGrantsInput {
	grants: FlowGrantInput[];
	confirm_self_lockout: boolean;
	dry_run: boolean;
	idempotency_key: string;
}

/** `PUT .../grants` response (`grants.rs::SetGrantsView`). `event_id` is `null` on a dry run. */
export interface FlowSetGrantsResponse {
	applied: boolean;
	event_id: string | null;
	permission_changes: FlowPermissionChanges;
}

/** `PUT /flow/objects/{object_id}/inheritance` body. `initial_grants` absent leaves the explicit
 * grants untouched; `[]` clears them (`routes/flow.rs::SetInheritanceRequest`). */
export interface SetFlowInheritanceInput {
	inherit_from_parent: boolean;
	confirm_self_lockout: boolean;
	dry_run: boolean;
	initial_grants?: FlowGrantInput[];
	idempotency_key: string;
}

/** `PUT .../inheritance` response (`grants.rs::SetInheritanceView`). */
export interface FlowSetInheritanceResponse extends FlowSetGrantsResponse {
	inherit_from_parent: boolean;
}

export interface FlowLinkPayload {
	target_object_id: string;
	relation_type: string;
	properties?: Record<string, unknown>;
	position_key?: string;
}
export interface FlowLinkCommandResult {
	relation: {
		relation_id: string;
		relation_type: string;
		source_object_id: string;
		target_object_id: string;
	};
	existing_document_cardinality: 0;
}
export interface FlowUnlinkCommandResult {
	removed: true;
	event_id: string;
	existing_document_cardinality: 0;
}

export type FlowRelationDirection = 'outgoing' | 'incoming' | 'both';

/** `RelationView` (`apps/api/src/flow/model.rs`): a tagged union on `visibility`. The
 * `unavailable` variant carries no other field at all. */
export type FlowRelationView =
	| {
			visibility: 'visible';
			relation_id: string;
			relation_type: string;
			direction: 'outgoing' | 'incoming';
			position_key?: string;
			properties: unknown;
			created_at: string;
			other_object: {
				scope_id?: string;
				id: string;
				object_type: string;
				title: string;
				lifecycle_status: string;
				project_id?: string;
			};
	  }
	| { visibility: 'unavailable' };

export interface FlowRelationListResponse {
	items: FlowRelationView[];
	next_cursor?: string;
}

export interface FlowRelationsQuery {
	direction?: FlowRelationDirection;
	relation_type?: string;
	cursor?: string;
	limit?: number;
}

export type FlowObjectExportFormat = 'json' | 'markdown' | 'csv' | 'package';

/** `POST /flow/objects/{object_id}/exports` body (`routes/flow.rs::PackageExportRequest`).
 * `include_history` is only accepted for `package` (`flow/export.rs::validate_request`). */
export interface CreateObjectExportInput {
	format: FlowObjectExportFormat;
	include_history: boolean;
	idempotency_key: string;
}

function buildQuery(params: Record<string, string | number | boolean | undefined>): string {
	const search = new URLSearchParams();
	for (const [key, value] of Object.entries(params)) {
		if (value !== undefined && value !== null && value !== '') {
			search.set(key, String(value));
		}
	}
	const qs = search.toString();
	return qs ? `?${qs}` : '';
}

export interface PortablePagePackagePreview {
	title: string;
	block_count: number;
	asset_count: number;
	asset_bytes: number;
	linked_page_count: number;
	package_sha256: string;
	blocks: Array<{ kind: string; text: string }>;
	assets: Array<{ filename: string; size: number }>;
}

function portablePagePackagePath(objectId: string): string {
	return `/api/v2/flow/objects/${encodeURIComponent(objectId)}/portable-page-package`;
}

export interface PortableGraphPagePreview {
	source_object_id: string;
	source_parent_id: string | null;
	title: string;
	version_count: number;
	source_sequences: number[];
	excerpts?: string[];
	asset_count: number;
}

export interface PortableGraphFormPreview {
	source_form_id: string;
	name: string;
}

export type PortableGraphReference = {
	source_object_id: string;
	mapping: 'new';
} & (
	| { kind: 'form'; target_form_id: string }
	| { kind: 'relation'; target_object_id: string; relation_type: string }
	| { kind: 'child_page'; target_object_id: string }
);

export interface PortableGraphPreview {
	package_sha256: string;
	pages: PortableGraphPagePreview[];
	forms: PortableGraphFormPreview[];
	include_history: boolean;
	references: PortableGraphReference[];
}

export interface PortableGraphReceipt {
	pages: Array<{
		source_object_id: string;
		object_id: string;
		document_id: string;
		versions: Array<{ source_seq: number; local_seq: number; source_recorded_at: string | null }>;
	}>;
	include_history: boolean;
	forms: Array<{ source_form_id: string; form_id: string }>;
}

function portableGraphPackagePath(objectId: string): string {
	return `/api/v2/flow/objects/${encodeURIComponent(objectId)}/portable-graph-package`;
}

export interface FlowCapabilities {
	block_transfers_enabled: boolean;
	portable_page_packages_enabled: boolean;
	portable_graph_packages_enabled: boolean;
	scope_native_legacy: boolean;
	personal_provisioning: boolean;
	personal_writes: boolean;
	external_mounts: boolean;
	personal_package_imports: boolean;
	personal_scope_package_exports: boolean;
	personal_forms_conversion: boolean;
	cross_workspace_forms_conversion: boolean;
}
export interface FlowScopeHome {
	scope: {
		id: string;
		kind: 'personal' | 'legacy_managed';
		owner_user_id?: string;
		lifecycle_status: string;
	};
	roots: Array<{ object_id: string; project_id?: string | null }>;
	capabilities?: FlowCapabilities;
}

export interface FlowMount {
	id: string;
	source_object_id: string;
	destination_workspace_id: string;
	destination_project_id?: string | null;
	level: 'view' | 'comment' | 'edit';
	status: string;
}
export const flowApi = {
	listSharePrincipals(
		scopeId: string,
		q: string
	): Promise<
		ApiResult<{ items: Array<{ principal_kind: 'user'; principal_id: string; label: string }> }>
	> {
		return apiClient.get(
			`/api/v2/flow/scopes/${scopeId}/share-principals?q=${encodeURIComponent(q)}`
		);
	},
	listMounts(objectId: string): Promise<ApiResult<{ items: FlowMount[] }>> {
		return apiClient.get(`/api/v2/flow/objects/${objectId}/mounts`);
	},
	createMount(
		objectId: string,
		input: {
			destination_workspace_id: string;
			destination_project_id?: string;
			level: 'view' | 'comment' | 'edit';
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowMount>> {
		return apiClient.post(`/api/v2/flow/objects/${objectId}/mounts`, input);
	},
	updateMount(
		objectId: string,
		mountId: string,
		input: {
			expected_level: FlowMount['level'];
			level: FlowMount['level'];
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowMount>> {
		return apiClient.patch(`/api/v2/flow/objects/${objectId}/mounts/${mountId}`, input);
	},
	removeMount(objectId: string, mountId: string): Promise<ApiResult<{ removed: boolean }>> {
		return apiClient.delete(`/api/v2/flow/objects/${objectId}/mounts/${mountId}`);
	},
	getCapabilities(): Promise<ApiResult<FlowCapabilities>> {
		return apiClient.get('/api/v2/flow/capabilities');
	},
	getHome(): Promise<ApiResult<FlowScopeHome>> {
		return apiClient.get('/api/v2/flow/home');
	},
	getWorkspaceScope(workspaceId: string): Promise<ApiResult<FlowScopeHome>> {
		return apiClient.get(`/api/v2/flow/workspaces/${workspaceId}/scope`);
	},
	createObject(
		workspaceId: string,
		input: CreateFlowObjectInput
	): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v2/flow/objects`, {
			...input,
			...(input.object_type === 'page' ? { editor_model: 5 } : {}),
			scope_id: workspaceId
		});
	},

	listObjects(
		workspaceId: string,
		query: ListFlowObjectsQuery = {}
	): Promise<ApiResult<FlowObjectListResponse>> {
		const qs = buildQuery({
			project_id: query.project_id,
			unprojected: query.unprojected,
			object_type: query.object_type,
			parent_id: query.parent_id,
			q: query.q,
			cursor: query.cursor,
			limit: query.limit,
			include_archived: query.include_archived
		});
		return apiClient.get<FlowObjectListResponse>(`/api/v2/flow/scopes/${workspaceId}/objects${qs}`);
	},

	getNavigator(
		workspaceId: string,
		query: { project_id?: string; depth?: number; include_archived?: boolean } = {}
	): Promise<ApiResult<FlowNavigatorResponse>> {
		const qs = buildQuery({
			project_id: query.project_id,
			depth: query.depth,
			include_archived: query.include_archived
		});
		return apiClient.get<FlowNavigatorResponse>(
			`/api/v2/flow/scopes/${workspaceId}/navigator${qs}`
		);
	},

	getObject(
		objectId: string,
		query: { at_seq?: number; render?: 'semantic_json' | 'markdown' } = {},
		signal?: AbortSignal
	): Promise<ApiResult<FlowObjectView>> {
		const qs = buildQuery({ at_seq: query.at_seq, render: query.render });
		return apiClient.request<FlowObjectView>(
			'GET',
			`/api/v2/flow/objects/${objectId}${qs}`,
			undefined,
			true,
			{},
			signal
		);
	},

	getHistory(
		objectId: string,
		query: { before_seq?: number; limit?: number } = {}
	): Promise<ApiResult<FlowHistoryResponse>> {
		const qs = buildQuery({ before_seq: query.before_seq, limit: query.limit ?? 20 });
		return apiClient.get<FlowHistoryResponse>(`/api/v2/flow/objects/${objectId}/history${qs}`);
	},

	createTicket(input: CreateTicketInput): Promise<ApiResult<CollabTicket>> {
		return apiClient.post<CollabTicket>('/api/v2/flow/collab/tickets', {
			...input,
			editor_model: 5
		});
	},

	getCollabDiagnostics(objectId: string): Promise<ApiResult<CollabDiagnostics>> {
		return apiClient.get<CollabDiagnostics>(
			`/api/v2/flow/objects/${objectId}/collab?include_sizes=true`
		);
	},

	/**
	 * `GET /workspaces/{workspace_id}/features/flow` (`rest-api-v1.md` v0.4 table; routed in
	 * `apps/api/src/main.rs`). Callers MUST treat any non-success `ApiResult` (including a
	 * transport-level failure) as `flow_enabled=false`: Flow is on by default on the server, but a
	 * failed read is not proof of that, so the client stays fail-closed.
	 */
	getFeatureFlags(workspaceId: string): Promise<ApiResult<FlowFeatureFlags>> {
		return apiClient.get<FlowFeatureFlags>(`/api/v1/workspaces/${workspaceId}/features/flow`);
	},

	/** `PUT /workspaces/{workspace_id}/features/flow` -- workspace admin only. */
	setFeatureFlags(
		workspaceId: string,
		input: SetFlowFeatureInput
	): Promise<ApiResult<FlowFeatureUpdate>> {
		return apiClient.put<FlowFeatureUpdate>(
			`/api/v1/workspaces/${workspaceId}/features/flow`,
			input
		);
	},

	executeCommand(
		objectId: string,
		input: ExecuteFlowCommandInput
	): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v2/flow/objects/${objectId}/commands`, input);
	},

	referenceObject(
		objectId: string,
		input: {
			target_type: 'form' | 'form_record';
			target_id: string;
			display?: Record<string, unknown>;
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowReferenceReceipt>> {
		return apiClient.post<FlowReferenceReceipt>(
			`/api/v2/flow/objects/${objectId}/references`,
			input
		);
	},

	listReferences(objectId: string): Promise<ApiResult<FlowReferenceList>> {
		return apiClient.get<FlowReferenceList>(`/api/v2/flow/objects/${objectId}/references`);
	},

	unreferenceObject(
		objectId: string,
		referenceId: string,
		idempotencyKey: string
	): Promise<ApiResult<{ removed: boolean; event_id: string }>> {
		return apiClient.deleteWithHeaders<{ removed: boolean; event_id: string }>(
			`/api/v2/flow/objects/${objectId}/references/${referenceId}`,
			{ 'Idempotency-Key': idempotencyKey }
		);
	},

	previewConversion(input: FlowConversionPreviewInput): Promise<ApiResult<FlowConversionPreview>> {
		return apiClient.post<FlowConversionPreview>('/api/v2/flow/conversions/preview', input);
	},

	commitConversion(input: FlowConversionCommitInput): Promise<ApiResult<FlowConversionJob>> {
		return apiClient.post<FlowConversionJob>('/api/v2/flow/conversions', input);
	},

	getConversion(jobId: string): Promise<ApiResult<FlowConversionJob>> {
		return apiClient.get<FlowConversionJob>(`/api/v2/flow/conversions/${jobId}`);
	},

	retryConversion(jobId: string, idempotencyKey: string): Promise<ApiResult<FlowConversionJob>> {
		return apiClient.post<FlowConversionJob>(`/api/v2/flow/conversions/${jobId}/retry`, {
			idempotency_key: idempotencyKey,
			confirm: true
		});
	},

	uploadPackageArtifact(
		workspaceId: string,
		file: Blob,
		filename: string,
		idempotencyKey: string,
		signal?: AbortSignal,
		onProgress?: (progress: UploadProgress) => void
	): Promise<ApiResult<FlowPackageArtifactReceipt>> {
		const body = new FormData();
		body.append('package', file, filename);
		// XHR when the runtime has it (browser upload progress), `fetch` otherwise -- see
		// `ApiClient.postFormDataWithProgress`.
		return apiClient.postFormDataWithProgress<FlowPackageArtifactReceipt>(
			`/api/v2/flow/scopes/${workspaceId}/import-artifacts`,
			body,
			{ 'Idempotency-Key': idempotencyKey },
			signal,
			onProgress
		);
	},

	exportWorkspace(
		workspaceId: string,
		input: CreateWorkspaceExportInput
	): Promise<ApiResult<FlowExportJob>> {
		return apiClient.post<FlowExportJob>(`/api/v2/flow/scopes/${workspaceId}/exports`, input);
	},

	startObjectExport(
		objectId: string,
		input: CreateObjectExportInput
	): Promise<ApiResult<FlowExportJob>> {
		return apiClient.post<FlowExportJob>(`/api/v2/flow/objects/${objectId}/exports`, input);
	},

	exportPortablePage(objectId: string) {
		return apiClient.getBinary(portablePagePackagePath(objectId));
	},

	previewPortablePage(objectId: string, archive: Blob, mode: 'export' | 'import' = 'import') {
		return apiClient.request<PortablePagePackagePreview>(
			'POST',
			`${portablePagePackagePath(objectId)}/preview${mode === 'export' ? '?mode=export' : ''}`,
			archive,
			true,
			{ 'Content-Type': 'application/zip' }
		);
	},

	importPortablePage(objectId: string, archive: Blob, sha256: string, key: string) {
		return apiClient.request<{ object_id: string; document_id: string; asset_count: number }>(
			'POST',
			portablePagePackagePath(objectId),
			archive,
			true,
			{ 'Content-Type': 'application/zip', 'X-Package-SHA256': sha256, 'Idempotency-Key': key }
		);
	},

	exportPortableGraph(
		objectId: string,
		input: { page_ids: string[]; form_ids: string[]; include_history: boolean }
	) {
		return apiClient.postBinary(`${portableGraphPackagePath(objectId)}/export`, input);
	},

	previewPortableGraph(objectId: string, archive: Blob, mode: 'export' | 'import' = 'import') {
		return apiClient.request<PortableGraphPreview>(
			'POST',
			`${portableGraphPackagePath(objectId)}/preview${mode === 'export' ? '?mode=export' : ''}`,
			archive,
			true,
			{ 'Content-Type': 'application/zip' }
		);
	},

	importPortableGraph(objectId: string, archive: Blob, sha256: string, key: string) {
		return apiClient.request<PortableGraphReceipt>(
			'POST',
			portableGraphPackagePath(objectId),
			archive,
			true,
			{ 'Content-Type': 'application/zip', 'X-Package-SHA256': sha256, 'Idempotency-Key': key }
		);
	},

	getExportJob(jobId: string): Promise<ApiResult<FlowExportJob>> {
		return apiClient.get<FlowExportJob>(`/api/v2/flow/exports/${jobId}`);
	},

	/**
	 * Downloads an export artifact through `download_url` (`GET /flow/exports/{job_id}/artifact`)
	 * with the caller's bearer token, so the download works for token-only sessions that carry no
	 * auth cookie. Only same-API relative paths are accepted; anything else is refused before a
	 * request is made. The server's `x-flow-package-sha256` header comes back as `sha256`.
	 */
	async downloadExportArtifact(
		downloadUrl: string
	): Promise<ApiResult<{ blob: Blob; sha256: string | null }>> {
		if (!/^\/api\/v[12]\/flow\/exports\/[^/?#]+\/artifact$/.test(downloadUrl)) {
			return transportFailure(400, 'transport.unsupportedDownloadUrl');
		}
		const result = await apiClient.getBinary(downloadUrl);
		if (result.code !== 0 || !result.data) return { ...result, data: null };
		return {
			code: 0,
			message: result.message,
			data: { blob: result.data.blob, sha256: result.data.headers.get('x-flow-package-sha256') }
		};
	},

	previewPackageImport(
		workspaceId: string,
		input: {
			artifact_id: string;
			project_mapping: Record<string, string | null>;
			external_reference_policy: FlowPackageExternalReferencePolicy;
			conflict_policy: FlowPackageConflictPolicy;
			include_history: boolean;
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowPackageImportPreview>> {
		return apiClient.post<FlowPackageImportPreview>(
			`/api/v2/flow/scopes/${workspaceId}/imports/preview`,
			input
		);
	},

	commitPackageImport(
		workspaceId: string,
		importId: string,
		input: {
			package_sha256: string;
			mapping_hash: string;
			conflict_policy: FlowPackageConflictPolicy;
			confirm: true;
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowPackageImportJobReceipt>> {
		return apiClient.post<FlowPackageImportJobReceipt>(
			`/api/v2/flow/scopes/${workspaceId}/imports/${importId}/commit`,
			input
		);
	},

	getPackageImport(
		workspaceId: string,
		importId: string
	): Promise<ApiResult<FlowPackageImportReport>> {
		return apiClient.get<FlowPackageImportReport>(
			`/api/v2/flow/scopes/${workspaceId}/imports/${importId}`
		);
	},

	/** `GET /workspaces/{id}/flow/search`. Takes an `AbortSignal` so the navigator can cancel the
	 * previous request when the query changes. */
	searchObjects(
		workspaceId: string,
		query: FlowSearchQuery,
		signal?: AbortSignal
	): Promise<ApiResult<FlowSearchResponse>> {
		const qs = buildQuery({
			q: query.q,
			project_id: query.project_id,
			unprojected: query.unprojected,
			all_visible: query.all_visible,
			object_type: query.object_type,
			freshness: query.freshness,
			cursor: query.cursor,
			limit: query.limit
		});
		return apiClient.request<FlowSearchResponse>(
			'GET',
			`/api/v2/flow/scopes/${workspaceId}/search${qs}`,
			undefined,
			true,
			{},
			signal
		);
	},

	/** The workspace's projection lag, read through the Flow scope bound to the workspace. */
	async getWorkspaceProjectionLag(
		workspaceId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowProjectionLag>> {
		const home = await flowApi.getWorkspaceScope(workspaceId);
		if (home.code !== 0 || !home.data) return { ...home, data: null };
		return flowApi.getProjectionLag(home.data.scope.id, query);
	},

	getAdminHealth(workspaceId: string): Promise<ApiResult<FlowAdminHealth>> {
		return apiClient.get<FlowAdminHealth>(`/api/v1/admin/workspaces/${workspaceId}/flow/health`);
	},
	/** Paginated policy-visible lag rows; aggregates cover the authorized scope before paging. */
	getProjectionLag(
		scopeId: string,
		query: { project_id?: string; cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowProjectionLag>> {
		return apiClient.get<FlowProjectionLag>(
			`/api/v2/flow/scopes/${scopeId}/projection-lag${buildQuery(query)}`
		);
	},

	/** One page of per-document lag rows (`cursor?`, `limit` 1..=100); the maxima are
	 * workspace-wide on every page. */
	getAdminLag(
		workspaceId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowAdminLag>> {
		const qs = buildQuery({ cursor: query.cursor, limit: query.limit });
		return apiClient.get<FlowAdminLag>(`/api/v1/admin/workspaces/${workspaceId}/flow/lag${qs}`);
	},

	/** One page of `documents[]` (`cursor?`, `limit` 1..=100); `counts` are workspace-wide
	 * regardless. */
	getAdminIntegrity(
		workspaceId: string,
		query: { scope: 'summary' | 'documents'; cursor?: string; limit?: number }
	): Promise<ApiResult<FlowAdminIntegrity>> {
		const qs = buildQuery({ scope: query.scope, cursor: query.cursor, limit: query.limit });
		return apiClient.get<FlowAdminIntegrity>(
			`/api/v1/admin/workspaces/${workspaceId}/flow/integrity${qs}`
		);
	},

	verifyDocument(
		documentId: string,
		input: VerifyDocumentInput
	): Promise<ApiResult<FlowOperationReceipt>> {
		return apiClient.post<FlowOperationReceipt>(
			`/api/v1/admin/flow/documents/${documentId}/verify`,
			input
		);
	},

	compactDocument(
		documentId: string,
		input: CompactDocumentInput
	): Promise<ApiResult<FlowOperationReceipt>> {
		return apiClient.post<FlowOperationReceipt>(
			`/api/v1/admin/flow/documents/${documentId}/compact`,
			input
		);
	},

	rebuildProjection(
		objectId: string,
		input: RebuildProjectionInput
	): Promise<ApiResult<FlowOperationReceipt>> {
		return apiClient.post<FlowOperationReceipt>(
			`/api/v1/admin/flow/objects/${objectId}/rebuild-projection`,
			input
		);
	},

	replayDeliveries(
		workspaceId: string,
		input: ReplayDeliveriesInput
	): Promise<ApiResult<FlowReplayResponse>> {
		return apiClient.post<FlowReplayResponse>(
			`/api/v1/admin/workspaces/${workspaceId}/flow/deliveries/replay`,
			input
		);
	},

	getBootstrap(
		objectId: string,
		known: { known_seq?: number; known_frontier?: string } = {}
	): Promise<ApiResult<FlowBootstrap>> {
		const qs = buildQuery({
			known_seq: known.known_seq,
			known_frontier: known.known_frontier,
			editor_model: 5
		});
		return apiClient.get<FlowBootstrap>(`/api/v2/flow/objects/${objectId}/bootstrap${qs}`);
	},

	getDiff(
		objectId: string,
		query: { from_seq: number; to_seq: number; render?: 'semantic_json' | 'markdown' }
	): Promise<ApiResult<FlowDiffResponse>> {
		const qs = buildQuery({ from_seq: query.from_seq, to_seq: query.to_seq, render: query.render });
		return apiClient.get<FlowDiffResponse>(`/api/v2/flow/objects/${objectId}/diff${qs}`);
	},

	getGrants(objectId: string): Promise<ApiResult<FlowGrantsView>> {
		return apiClient.get<FlowGrantsView>(`/api/v2/flow/objects/${objectId}/grants`);
	},

	setGrants(
		objectId: string,
		input: SetFlowGrantsInput
	): Promise<ApiResult<FlowSetGrantsResponse>> {
		return apiClient.put<FlowSetGrantsResponse>(`/api/v2/flow/objects/${objectId}/grants`, input);
	},

	setInheritance(
		objectId: string,
		input: SetFlowInheritanceInput
	): Promise<ApiResult<FlowSetInheritanceResponse>> {
		return apiClient.put<FlowSetInheritanceResponse>(
			`/api/v2/flow/objects/${objectId}/inheritance`,
			input
		);
	},

	/** Metadata-only relation commands echo the accepted source head; no new doc update. */
	linkObjects(
		objectId: string,
		payload: FlowLinkPayload,
		idempotencyKey: string
	): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v2/flow/objects/${objectId}/commands`, {
			command: { type: 'link', payload },
			idempotency_key: idempotencyKey
		});
	},
	unlinkObject(
		objectId: string,
		relationId: string,
		idempotencyKey: string
	): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(`/api/v2/flow/objects/${objectId}/commands`, {
			command: { type: 'unlink', payload: { relation_id: relationId } },
			idempotency_key: idempotencyKey
		});
	},

	getRelations(
		objectId: string,
		query: FlowRelationsQuery = {}
	): Promise<ApiResult<FlowRelationListResponse>> {
		const qs = buildQuery({
			direction: query.direction,
			relation_type: query.relation_type,
			cursor: query.cursor,
			limit: query.limit
		});
		return apiClient.get<FlowRelationListResponse>(
			`/api/v2/flow/objects/${objectId}/relations${qs}`
		);
	},

	// v0.6 Collections (`rest-api-v1.md` v0.6 table; routes in `apps/api/src/main.rs`).

	/** `GET /flow/objects/{id}/schema`. Served by the same handler as `getCollection`
	 * (`main.rs` routes both to `get_flow_collection`), so a non-Collection id is `not_found`. */
	getObjectSchema(objectId: string): Promise<ApiResult<FlowCollectionDescription>> {
		return apiClient.get<FlowCollectionDescription>(`/api/v2/flow/objects/${objectId}/schema`);
	},

	getRecordContext(recordId: string): Promise<ApiResult<FlowRecordContext>> {
		return apiClient.get<FlowRecordContext>(`/api/v2/flow/records/${recordId}/context`);
	},

	getCollection(collectionId: string): Promise<ApiResult<FlowCollectionDescription>> {
		return apiClient.get<FlowCollectionDescription>(`/api/v2/flow/collections/${collectionId}`);
	},

	listCollectionRecords(
		collectionId: string,
		query: FlowCollectionRecordsQuery = {}
	): Promise<ApiResult<FlowCollectionRecordsResponse>> {
		const qs = buildQuery({
			view_id: query.view_id,
			filter: query.filter ? JSON.stringify(query.filter) : undefined,
			sort: query.sort ? JSON.stringify(query.sort) : undefined,
			group: query.group,
			cursor: query.cursor,
			limit: query.limit,
			field_ids:
				query.field_ids && query.field_ids.length > 0 ? query.field_ids.join(',') : undefined
		});
		return apiClient.get<FlowCollectionRecordsResponse>(
			`/api/v2/flow/collections/${collectionId}/records${qs}`
		);
	},

	queryCollectionRecords(
		collectionId: string,
		input: FlowCollectionQueryInput
	): Promise<ApiResult<FlowCollectionRecordsResponse>> {
		return apiClient.post<FlowCollectionRecordsResponse>(
			`/api/v2/flow/collections/${collectionId}/query`,
			input
		);
	},

	createCollectionRecord(
		collectionId: string,
		input: CreateCollectionRecordInput
	): Promise<ApiResult<AcceptedChange>> {
		return apiClient.post<AcceptedChange>(
			`/api/v2/flow/collections/${collectionId}/records`,
			input
		);
	},

	listComments(objectId: string): Promise<ApiResult<FlowCommentList>> {
		return apiClient.get<FlowCommentList>(`/api/v2/flow/objects/${objectId}/comments`);
	},

	createComment(
		objectId: string,
		input: CreateFlowCommentInput,
		idempotencyKey?: string
	): Promise<ApiResult<FlowComment>> {
		return apiClient.request<FlowComment>(
			'POST',
			`/api/v2/flow/objects/${objectId}/comments`,
			input,
			true,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	updateComment(
		objectId: string,
		commentId: string,
		input: { body: string; mentions?: string[] },
		idempotencyKey?: string
	): Promise<ApiResult<FlowComment>> {
		return apiClient.request<FlowComment>(
			'PATCH',
			`/api/v2/flow/objects/${objectId}/comments/${commentId}`,
			input,
			true,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	deleteComment(
		objectId: string,
		commentId: string,
		idempotencyKey?: string
	): Promise<ApiResult<{ deleted: boolean }>> {
		return apiClient.deleteWithHeaders<{ deleted: boolean }>(
			`/api/v2/flow/objects/${objectId}/comments/${commentId}`,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	resolveComment(
		objectId: string,
		commentId: string,
		resolved: boolean,
		idempotencyKey?: string
	): Promise<ApiResult<FlowComment>> {
		return apiClient.request<FlowComment>(
			'POST',
			`/api/v2/flow/objects/${objectId}/comments/${commentId}/resolve`,
			{ resolved },
			true,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	listShareLinks(objectId: string): Promise<ApiResult<FlowShareLinkList>> {
		return apiClient.get<FlowShareLinkList>(`/api/v2/flow/objects/${objectId}/share-links`);
	},

	createShareLink(
		objectId: string,
		input: { expires_at?: string | null }
	): Promise<ApiResult<CreatedFlowShareLink>> {
		return apiClient.post<CreatedFlowShareLink>(
			`/api/v2/flow/objects/${objectId}/share-links`,
			input
		);
	},

	revokeShareLink(objectId: string, linkId: string): Promise<ApiResult<FlowShareLink>> {
		return apiClient.post<FlowShareLink>(
			`/api/v2/flow/objects/${objectId}/share-links/${linkId}/revoke`,
			{}
		);
	},

	getPublicShare(token: string): Promise<ApiResult<FlowPublicShare>> {
		return apiClient.get<FlowPublicShare>(
			`/api/v2/flow/public-shares/${encodeURIComponent(token)}`
		);
	},

	bodyAssetLimits(objectId: string, signal?: AbortSignal): Promise<ApiResult<BodyAssetLimits>> {
		return apiClient.request(
			'GET',
			`/api/v2/flow/objects/${objectId}/body-assets/limits`,
			undefined,
			true,
			{},
			signal
		);
	},
	uploadBodyAsset(
		objectId: string,
		kind: BodyAssetKind,
		file: File,
		key: string,
		signal: AbortSignal,
		progress: (value: UploadProgress) => void
	): Promise<ApiResult<BodyAssetReceipt>> {
		const data = new FormData();
		data.set('kind', kind);
		data.set('file', file, file.name);
		return apiClient.postFormDataWithProgress(
			`/api/v2/flow/objects/${objectId}/body-assets`,
			data,
			{ 'Idempotency-Key': key },
			signal,
			progress
		);
	},
	bodyAsset(objectId: string, assetId: string, signal?: AbortSignal) {
		return apiClient
			.getBinary(`/api/v2/flow/objects/${objectId}/body-assets/${assetId}`, true, signal)
			.then(bodyBinaryResult);
	},
	publicBodyAsset(token: string, assetId: string, signal?: AbortSignal) {
		return apiClient
			.getBinary(
				`/api/v2/flow/public-shares/${encodeURIComponent(token)}/body-assets/${assetId}`,
				true,
				signal
			)
			.then(bodyBinaryResult);
	},
	discardBodyAsset(objectId: string, assetId: string): Promise<ApiResult<unknown>> {
		return apiClient.deleteWithHeaders(`/api/v2/flow/objects/${objectId}/body-assets/${assetId}`, {
			'Idempotency-Key': `discard-asset:${assetId}`
		});
	},
	prepareChildPage(
		objectId: string,
		title: string,
		key: string,
		signal?: AbortSignal
	): Promise<ApiResult<FlowChildReservation>> {
		return apiClient.request(
			'POST',
			`/api/v2/flow/objects/${objectId}/child-reservations`,
			{ title },
			true,
			{ 'Idempotency-Key': key },
			signal
		);
	},
	childReservationStatus(
		objectId: string,
		reservationId: string,
		signal?: AbortSignal
	): Promise<ApiResult<FlowChildReservation>> {
		return apiClient.request(
			'GET',
			`/api/v2/flow/objects/${objectId}/child-reservations/${reservationId}`,
			undefined,
			true,
			{},
			signal
		);
	},

	cancelChildReservation(objectId: string, reservationId: string): Promise<ApiResult<unknown>> {
		return apiClient.deleteWithHeaders(
			`/api/v2/flow/objects/${objectId}/child-reservations/${reservationId}`,
			{ 'Idempotency-Key': `cancel-child:${reservationId}` }
		);
	},

	setAppearance(
		objectId: string,
		appearance: FlowAppearance,
		idempotencyKey?: string
	): Promise<ApiResult<FlowAppearance>> {
		return apiClient.request<FlowAppearance>(
			'PUT',
			`/api/v2/flow/objects/${objectId}/appearance`,
			appearance,
			true,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	uploadAppearance(
		objectId: string,
		dataBase64: string,
		idempotencyKey?: string
	): Promise<ApiResult<{ asset_id: string; content_type: string; byte_size: number }>> {
		return apiClient.request(
			'POST',
			`/api/v2/flow/objects/${objectId}/appearance/assets`,
			{ data_base64: dataBase64 },
			true,
			idempotencyKey ? { 'Idempotency-Key': idempotencyKey } : {}
		);
	},

	appearanceAsset(objectId: string, assetId: string) {
		return apiClient.getBinary(`/api/v2/flow/objects/${objectId}/appearance/assets/${assetId}`);
	},

	restoreVersion(
		objectId: string,
		input: { seq: number; idempotency_key: string; message?: string }
	): Promise<ApiResult<FlowRestoreResult>> {
		return apiClient.post<FlowRestoreResult>(
			`/api/v2/flow/objects/${objectId}/history/restore`,
			input
		);
	},

	/** Archived policy-visible objects, with a stable cursor and limit1..100. */
	listTrash(
		scopeId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowTrashPage>> {
		return apiClient.get<FlowTrashPage>(`/api/v2/flow/scopes/${scopeId}/trash${buildQuery(query)}`);
	},
	/** The workspace's archived objects, read through the Flow scope bound to the workspace. */
	async listWorkspaceTrash(
		workspaceId: string,
		query: { cursor?: string; limit?: number } = {}
	): Promise<ApiResult<FlowTrashPage>> {
		const home = await flowApi.getWorkspaceScope(workspaceId);
		if (home.code !== 0 || !home.data) return { ...home, data: null };
		return flowApi.listTrash(home.data.scope.id, query);
	},

	purgeObject(objectId: string): Promise<ApiResult<{ deleted_id: string; deleted_count: number }>> {
		return apiClient.post(`/api/v2/flow/objects/${objectId}/purge`, {});
	},

	listMentionCandidates(
		objectId: string,
		q = ''
	): Promise<ApiResult<{ items: FlowMentionCandidate[] }>> {
		const suffix = q ? `?q=${encodeURIComponent(q)}` : '';
		return apiClient.get(`/api/v2/flow/objects/${objectId}/mention-candidates${suffix}`);
	}
};

let appearanceCacheToken: string | null = null;
const appearanceBlobs = new Map<string, Promise<string | null>>();

/** Authenticated image URL for an icon or cover. Cached for the page session. */
export function clearFlowAppearanceCache(objectId?: string): void {
	for (const [key, promise] of appearanceBlobs) {
		if (!objectId || key.startsWith(`${objectId}:`)) {
			appearanceBlobs.delete(key);
			void promise.then((url) => {
				if (url) URL.revokeObjectURL(url);
			});
		}
	}
}

export function appearanceBlobUrl(objectId: string, assetId: string): Promise<string | null> {
	const token = apiClient.getToken();
	if (token !== appearanceCacheToken) {
		clearFlowAppearanceCache();
		appearanceCacheToken = token;
	}
	const key = `${objectId}:${assetId}`;
	const cached = appearanceBlobs.get(key);
	if (cached) return cached;
	const pending = flowApi.appearanceAsset(objectId, assetId).then((result) => {
		if (result.code !== 0 || !result.data) return null;
		return URL.createObjectURL(result.data.blob);
	});
	appearanceBlobs.set(key, pending);
	return pending;
}

export function publicAppearanceUrl(token: string, assetId: string): string {
	const base = import.meta.env.VITE_API_BASE_URL ?? '';
	return `${base}/api/v2/flow/public-shares/${encodeURIComponent(token)}/assets/${encodeURIComponent(assetId)}`;
}
