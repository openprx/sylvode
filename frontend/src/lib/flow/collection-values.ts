// Collection schema parsing and typed value encode/decode (`versions/v0.6-collections.md`
// "Web 交互": "字段类型化编辑"; `rest-api-v1.md` v0.6). Pure functions only: no network, no
// engine doc, no Svelte, so every rule below is unit-testable in bun.
//
// Wire facts this module follows (backend source, not the contract sketch):
// - field types are exactly `text|number|boolean|date|select|multi_select|relation`
//   (`apps/api/src/flow/collections.rs::validate_field_type`); a read serializes the type as
//   `type` (`CollectionFieldView`);
// - a value must be the JSON type `validate_typed_value` accepts: `text|select|date` a string,
//   `number` a JSON number (a numeric STRING is `invalid_update`), `boolean` a boolean,
//   `multi_select` an array of strings, `relation` an array of UUID strings; `null` clears;
// - a `date` string is cast with `::timestamptz` into the typed index
//   (`sync_record_projection`), and an unparseable one fails that cast as an untyped HTTP 500,
//   so the client only ever sends a validated calendar date `YYYY-MM-DD`;
// - the server attaches no meaning to `config` beyond `config.restricted`; select options are
//   this client's convention `config.options: string[]` and the stored value is the option text;
// - restricted fields are removed from `fields` and from `values_by_field_id` by the server
//   (`describe_collection`, `query_collection_records_inner`). A missing or malformed value is
//   shown as a placeholder, never recovered from another representation (`ADR-0018` NR-7).

import type {
	FlowCollectionDescription,
	FlowCollectionFieldType,
	FlowCollectionFieldView,
	FlowCollectionViewView,
	FlowRecordFilter,
	FlowRecordSort
} from '$lib/api/flow';

export const COLLECTION_FIELD_TYPES = [
	'text',
	'number',
	'boolean',
	'date',
	'select',
	'multi_select',
	'relation'
] as const satisfies readonly FlowCollectionFieldType[];

/** Types the schema mode offers for a new field. `relation` needs a target picker that this
 * release does not ship, so it is displayed read-only but not created from the UI. */
export const CREATABLE_FIELD_TYPES = [
	'text',
	'number',
	'boolean',
	'date',
	'select',
	'multi_select'
] as const satisfies readonly FlowCollectionFieldType[];

export type CollectionViewKind = 'table' | 'board';

/** `flow/collections.rs::LABEL_MAX_CHARS`. */
export const COLLECTION_LABEL_MAX_CHARS = 500;

export interface CollectionField {
	readonly fieldId: string;
	readonly label: string;
	/** `null` when the server sent a type this client does not know: shown read-only. */
	readonly type: FlowCollectionFieldType | null;
	readonly config: Readonly<Record<string, unknown>>;
	readonly position: number;
	readonly restricted: boolean;
	readonly options: readonly string[];
}

export interface CollectionViewQueryConfig {
	readonly filter: FlowRecordFilter | null;
	readonly sort: FlowRecordSort | null;
	/** Board grouping field (`config.group_field_id`). */
	readonly groupFieldId: string | null;
	readonly columnWidths: Readonly<Record<string, number>>;
}

export interface CollectionView {
	readonly viewId: string;
	readonly name: string;
	readonly kind: CollectionViewKind | null;
	readonly config: Readonly<Record<string, unknown>>;
	readonly position: number;
	readonly query: CollectionViewQueryConfig;
}

export interface ParsedCollection {
	readonly collectionId: string;
	readonly title: string;
	readonly frontier: string;
	readonly schemaSeq: number;
	readonly recordCount: number;
	readonly fieldSecrecyEnabled: boolean;
	/** Live (non-archived) fields in `position` order. */
	readonly fields: readonly CollectionField[];
	readonly views: readonly CollectionView[];
	/** Ids of the Collection document's root nodes (fields and views share that parent) in
	 * document order, from the description's `object.semantic_content`. `field_reorder` /
	 * `view_reorder` take an index into THIS list (`collections.rs` emits a `MoveNode` with
	 * `new_parent: None`), not into the field or view list alone. */
	readonly rootOrder: readonly string[];
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isFieldType(value: unknown): value is FlowCollectionFieldType {
	return typeof value === 'string' && (COLLECTION_FIELD_TYPES as readonly string[]).includes(value);
}

/** `config.restricted`: the server treats anything other than absent/`false` as restricted
 * (`collections.rs::field_is_restricted`), so this does too. */
export function isRestrictedConfig(config: Readonly<Record<string, unknown>>): boolean {
	return 'restricted' in config && config.restricted !== false;
}

export function selectOptions(config: Readonly<Record<string, unknown>>): string[] {
	const raw = config.options;
	if (!Array.isArray(raw)) return [];
	const out: string[] = [];
	for (const option of raw) {
		if (typeof option === 'string' && option.trim() !== '' && !out.includes(option)) {
			out.push(option);
		}
	}
	return out;
}

export function parseField(raw: FlowCollectionFieldView): CollectionField {
	const config = isRecord(raw.config) ? raw.config : {};
	return {
		fieldId: raw.field_id,
		label: raw.label,
		type: isFieldType(raw.type) ? raw.type : null,
		config,
		position: raw.position,
		restricted: isRestrictedConfig(config),
		options: selectOptions(config)
	};
}

function parseFilter(raw: unknown): FlowRecordFilter | null {
	if (!isRecord(raw)) return null;
	if (typeof raw.field_id !== 'string' || typeof raw.op !== 'string' || !('value' in raw)) {
		return null;
	}
	return { field_id: raw.field_id, op: raw.op, value: raw.value };
}

function parseSort(raw: unknown): FlowRecordSort | null {
	if (!isRecord(raw) || typeof raw.field_id !== 'string') return null;
	const direction = raw.direction === 'desc' ? 'desc' : 'asc';
	return { field_id: raw.field_id, direction };
}

function parseWidths(raw: unknown): Record<string, number> {
	if (!isRecord(raw)) return {};
	const out: Record<string, number> = {};
	for (const [key, value] of Object.entries(raw)) {
		if (typeof value === 'number' && Number.isFinite(value)) out[key] = clampColumnWidth(value);
	}
	return out;
}

export function parseView(raw: FlowCollectionViewView): CollectionView {
	const config = isRecord(raw.config) ? raw.config : {};
	return {
		viewId: raw.view_id,
		name: raw.name,
		kind: raw.type === 'table' || raw.type === 'board' ? raw.type : null,
		config,
		position: raw.position,
		query: {
			filter: parseFilter(config.filter),
			sort: parseSort(config.sort),
			groupFieldId: typeof config.group_field_id === 'string' ? config.group_field_id : null,
			columnWidths: parseWidths(config.column_widths)
		}
	};
}

/** Root node order of the Collection document (`SemanticSnapshot.nodes`, sorted by
 * `(order_key, id)` the way `sync_collection_projection` sorts), live nodes only. */
export function collectionRootOrder(semanticContent: unknown): string[] {
	if (!isRecord(semanticContent) || !isRecord(semanticContent.nodes)) return [];
	const entries: Array<[string, string]> = [];
	for (const [id, node] of Object.entries(semanticContent.nodes)) {
		if (!isRecord(node) || node.deleted === true) continue;
		if (node.parent !== null && node.parent !== undefined) continue;
		if (node.kind !== 'collection_field' && node.kind !== 'collection_view') continue;
		entries.push([id, typeof node.order_key === 'string' ? node.order_key : '']);
	}
	entries.sort(([idA, keyA], [idB, keyB]) =>
		keyA < keyB ? -1 : keyA > keyB ? 1 : idA < idB ? -1 : idA > idB ? 1 : 0
	);
	return entries.map(([id]) => id);
}

export function parseCollection(raw: FlowCollectionDescription): ParsedCollection {
	const fields = raw.fields
		.filter((field) => !field.archived)
		.map(parseField)
		.sort((a, b) => a.position - b.position || (a.fieldId < b.fieldId ? -1 : 1));
	const views = raw.views
		.map(parseView)
		.sort((a, b) => a.position - b.position || (a.viewId < b.viewId ? -1 : 1));
	return {
		collectionId: raw.object.id,
		title: raw.object.title,
		frontier: raw.object.frontier,
		schemaSeq: raw.schema_seq,
		recordCount: raw.record_count,
		fieldSecrecyEnabled: raw.field_secrecy_enabled,
		fields,
		views,
		rootOrder: collectionRootOrder(raw.object.semantic_content)
	};
}

/**
 * The `index` for a `field_reorder`/`view_reorder` that moves `movingId` one step past its
 * neighbour `neighbourId` (in either direction). The server applies `MoveNode` with the final
 * position among the root's children, so the moved node lands exactly where the neighbour is now.
 * `null` when either id is not a live root node (the move is then not sent).
 */
export function reorderIndex(
	rootOrder: readonly string[],
	movingId: string,
	neighbourId: string
): number | null {
	if (!rootOrder.includes(movingId)) return null;
	const index = rootOrder.indexOf(neighbourId);
	return index < 0 ? null : index;
}

// ---- column widths --------------------------------------------------------------------------

export const COLUMN_WIDTH_MIN = 96;
export const COLUMN_WIDTH_MAX = 640;
export const COLUMN_WIDTH_DEFAULT = 180;

export function clampColumnWidth(width: number): number {
	if (!Number.isFinite(width)) return COLUMN_WIDTH_DEFAULT;
	return Math.round(Math.min(COLUMN_WIDTH_MAX, Math.max(COLUMN_WIDTH_MIN, width)));
}

// ---- typed values ---------------------------------------------------------------------------

/** The editor-side form of a value: what an input element holds. */
export type DraftValue = string | boolean | readonly string[];

export type EncodeFailure =
	| 'number_invalid'
	| 'date_invalid'
	| 'option_unknown'
	| 'read_only'
	| 'type_mismatch';

export type EncodeResult =
	| { readonly ok: true; readonly value: unknown }
	| { readonly ok: false; readonly reason: EncodeFailure };

export const ENCODE_FAILURE_KEYS: Readonly<Record<EncodeFailure, string>> = {
	number_invalid: 'flow.collection.value.invalidNumber',
	date_invalid: 'flow.collection.value.invalidDate',
	option_unknown: 'flow.collection.value.unknownOption',
	read_only: 'flow.collection.value.readOnly',
	type_mismatch: 'flow.collection.value.invalid'
};

const NUMBER_PATTERN = /^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/;
const DATE_PATTERN = /^(\d{4})-(\d{2})-(\d{2})$/;
const UUID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** A real calendar date in `YYYY-MM-DD` (rejects `2026-02-30`). */
export function isCalendarDate(raw: string): boolean {
	const match = DATE_PATTERN.exec(raw);
	if (!match) return false;
	const [year, month, day] = [Number(match[1]), Number(match[2]), Number(match[3])];
	if (month < 1 || month > 12 || day < 1) return false;
	const probe = new Date(Date.UTC(year, month - 1, day));
	return (
		probe.getUTCFullYear() === year &&
		probe.getUTCMonth() === month - 1 &&
		probe.getUTCDate() === day
	);
}

/**
 * Editor value -> wire value for one field. An empty text/number/date/select input clears the
 * value (`null`); the server stores `null` without typing it (`validate_properties_against_fields`
 * skips it) and the projection omits it.
 */
export function encodeValue(field: CollectionField, draft: DraftValue): EncodeResult {
	switch (field.type) {
		case 'text': {
			if (typeof draft !== 'string') return { ok: false, reason: 'type_mismatch' };
			return { ok: true, value: draft === '' ? null : draft };
		}
		case 'number': {
			if (typeof draft !== 'string') return { ok: false, reason: 'type_mismatch' };
			const trimmed = draft.trim();
			if (trimmed === '') return { ok: true, value: null };
			if (!NUMBER_PATTERN.test(trimmed)) return { ok: false, reason: 'number_invalid' };
			const parsed = Number(trimmed);
			if (!Number.isFinite(parsed)) return { ok: false, reason: 'number_invalid' };
			return { ok: true, value: parsed };
		}
		case 'boolean': {
			if (typeof draft !== 'boolean') return { ok: false, reason: 'type_mismatch' };
			return { ok: true, value: draft };
		}
		case 'date': {
			if (typeof draft !== 'string') return { ok: false, reason: 'type_mismatch' };
			const trimmed = draft.trim();
			if (trimmed === '') return { ok: true, value: null };
			if (!isCalendarDate(trimmed)) return { ok: false, reason: 'date_invalid' };
			return { ok: true, value: trimmed };
		}
		case 'select': {
			if (typeof draft !== 'string') return { ok: false, reason: 'type_mismatch' };
			if (draft === '') return { ok: true, value: null };
			if (!field.options.includes(draft)) return { ok: false, reason: 'option_unknown' };
			return { ok: true, value: draft };
		}
		case 'multi_select': {
			if (!Array.isArray(draft)) return { ok: false, reason: 'type_mismatch' };
			const values = draft as readonly string[];
			if (values.some((value) => !field.options.includes(value))) {
				return { ok: false, reason: 'option_unknown' };
			}
			// Option order, not click order: the same set always encodes to the same array.
			return { ok: true, value: field.options.filter((option) => values.includes(option)) };
		}
		default:
			return { ok: false, reason: 'read_only' };
	}
}

/** Wire value -> editor value. A value of the wrong wire type yields `null` (the cell is shown
 * as invalid and editing starts from empty) rather than a coerced guess. */
export function decodeValue(field: CollectionField, value: unknown): DraftValue | null {
	switch (field.type) {
		case 'text':
			return value === undefined || value === null ? '' : typeof value === 'string' ? value : null;
		case 'number':
			return value === undefined || value === null
				? ''
				: typeof value === 'number' && Number.isFinite(value)
					? String(value)
					: null;
		case 'boolean':
			return value === undefined || value === null
				? false
				: typeof value === 'boolean'
					? value
					: null;
		case 'date':
			if (value === undefined || value === null) return '';
			return typeof value === 'string' && isCalendarDate(value) ? value : null;
		case 'select':
			return value === undefined || value === null ? '' : typeof value === 'string' ? value : null;
		case 'multi_select':
			if (value === undefined || value === null) return [];
			return Array.isArray(value) && value.every((item) => typeof item === 'string')
				? (value as string[])
				: null;
		default:
			return null;
	}
}

export type CellDisplay =
	| { readonly kind: 'restricted' }
	| { readonly kind: 'empty' }
	| { readonly kind: 'invalid' }
	| { readonly kind: 'text'; readonly text: string }
	| { readonly kind: 'boolean'; readonly value: boolean }
	| { readonly kind: 'options'; readonly values: readonly string[] }
	| { readonly kind: 'relation'; readonly ids: readonly string[] };

/**
 * What a cell shows. Only `values_by_field_id` is consulted: a key that is absent (not sent,
 * restricted, or never set) is `empty`/`restricted`, and a value of the wrong wire type is
 * `invalid`. Neither is ever filled in from `record.semantic_content` or any cache.
 */
export function displayValue(
	field: CollectionField,
	values: Readonly<Record<string, unknown>>
): CellDisplay {
	if (field.restricted) return { kind: 'restricted' };
	if (!Object.prototype.hasOwnProperty.call(values, field.fieldId)) return { kind: 'empty' };
	const value = values[field.fieldId];
	if (value === null || value === undefined) return { kind: 'empty' };
	switch (field.type) {
		case 'text':
			return typeof value === 'string'
				? value === ''
					? { kind: 'empty' }
					: { kind: 'text', text: value }
				: { kind: 'invalid' };
		case 'number':
			return typeof value === 'number' && Number.isFinite(value)
				? { kind: 'text', text: String(value) }
				: { kind: 'invalid' };
		case 'date':
			// Shown as stored: a date-time written by another client is not truncated or reformatted.
			return typeof value === 'string' ? { kind: 'text', text: value } : { kind: 'invalid' };
		case 'boolean':
			return typeof value === 'boolean' ? { kind: 'boolean', value } : { kind: 'invalid' };
		case 'select':
			return typeof value === 'string' ? { kind: 'options', values: [value] } : { kind: 'invalid' };
		case 'multi_select':
			return Array.isArray(value) && value.every((item) => typeof item === 'string')
				? value.length === 0
					? { kind: 'empty' }
					: { kind: 'options', values: value as string[] }
				: { kind: 'invalid' };
		case 'relation':
			return Array.isArray(value) &&
				value.every((item) => typeof item === 'string' && UUID_PATTERN.test(item))
				? value.length === 0
					? { kind: 'empty' }
					: { kind: 'relation', ids: value as string[] }
				: { kind: 'invalid' };
		default:
			return { kind: 'invalid' };
	}
}

export const CELL_PLACEHOLDER_KEYS = {
	restricted: 'flow.collection.value.restricted',
	empty: 'flow.collection.value.empty',
	invalid: 'flow.collection.value.invalid'
} as const;

/** Filter operators the server accepts per type (`query_collection_records_inner`). */
export function filterOperators(type: FlowCollectionFieldType | null): readonly string[] {
	switch (type) {
		case 'multi_select':
		case 'relation':
			return ['eq', 'ne', 'contains'];
		case 'boolean':
		case 'select':
		case 'text':
			return ['eq', 'ne'];
		case 'number':
		case 'date':
			return ['eq', 'ne', 'lt', 'lte', 'gt', 'gte'];
		default:
			return [];
	}
}

/** A filter's `value` must be the field's wire type (`validate_typed_value(filter.value)`);
 * `contains` on a `multi_select` takes an array. `null` is never a valid filter value. */
export function encodeFilterValue(
	field: CollectionField,
	op: string,
	draft: DraftValue
): EncodeResult {
	if (!filterOperators(field.type).includes(op)) return { ok: false, reason: 'type_mismatch' };
	if (field.type === 'multi_select') {
		const encoded = encodeValue(
			field,
			typeof draft === 'string' ? (draft === '' ? [] : [draft]) : draft
		);
		if (!encoded.ok) return encoded;
		return Array.isArray(encoded.value) && encoded.value.length > 0
			? encoded
			: { ok: false, reason: 'option_unknown' };
	}
	const encoded = encodeValue(field, draft);
	if (!encoded.ok) return encoded;
	if (encoded.value === null) {
		return {
			ok: false,
			reason:
				field.type === 'number'
					? 'number_invalid'
					: field.type === 'date'
						? 'date_invalid'
						: 'option_unknown'
		};
	}
	return encoded;
}

/** The editor's starting value for an empty field. */
export function emptyDraft(field: CollectionField): DraftValue {
	switch (field.type) {
		case 'boolean':
			return false;
		case 'multi_select':
			return [];
		default:
			return '';
	}
}

/** What an editor shows: the uncommitted draft, else the decoded stored value, else empty. */
export function editorValue(
	field: CollectionField,
	values: Readonly<Record<string, unknown>>,
	draft: DraftValue | null
): DraftValue {
	if (draft !== null) return draft;
	if (field.restricted) return emptyDraft(field);
	return decodeValue(field, values[field.fieldId]) ?? emptyDraft(field);
}

/** Whether a field can be edited from this client (known, non-relation, not restricted). */
export function isEditableField(field: CollectionField): boolean {
	return field.type !== null && field.type !== 'relation' && !field.restricted;
}
