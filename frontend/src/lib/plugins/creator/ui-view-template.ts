import type {
	PluginContributionV3,
	PluginManifestV3,
	PluginReadV3,
	PluginSlotV3,
	PluginUploadV3
} from '../../api/plugin-packages';
import type { CreatorOrigin, CreatorTemplate, PreviewTransport } from './registry';
import { sha256 } from './template';
import html from './ui-view-template.html?raw';

export type UiPresentation = 'table' | 'summary';

export interface UiViewContributionDraft {
	slot: PluginSlotV3;
	enabled: boolean;
	label: string;
	route: string;
	formKey: string;
}

export interface UiViewDraft {
	key: string;
	name: string;
	publisher: string;
	version: string;
	contributions: UiViewContributionDraft[];
	readFormKey: string;
	/** Comma or space separated field keys. */
	readFields: string;
	presentation: UiPresentation;
	title: string;
	text: string;
}

export const uiViewSlots: PluginSlotV3[] = [
	'project.view',
	'form.list.toolbar',
	'form.record.detail',
	'form.record.actions'
];

const slotMeta: Record<PluginSlotV3, { id: string; kind: PluginContributionV3['kind']; view: string }> = {
	'project.view': { id: 'project_view', kind: 'view', view: 'view' },
	'form.list.toolbar': { id: 'list_toolbar', kind: 'action', view: 'toolbar' },
	'form.record.detail': { id: 'record_detail', kind: 'panel', view: 'detail' },
	'form.record.actions': { id: 'record_actions', kind: 'action', view: 'actions' }
};

export const readId = 'records';
export const sampleRecordId = '00000000-0000-4000-8000-000000000001';
export const maxHtmlBytes = 65536;
const keyPattern = /^[a-z][a-z0-9_]*$/;
const releasePattern = /^(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})\.(0|[1-9]\d{0,8})$/;
const control = /[\u0000-\u001f\u007f-\u009f]/;
const encoder = new TextEncoder();
const defaultFormKey = 'order_line';
const defaultLabels: Record<PluginSlotV3, string> = {
	'project.view': 'Data view',
	'form.list.toolbar': 'Data summary',
	'form.record.detail': 'Record data',
	'form.record.actions': 'Record data'
};

export function parseFields(value: string): string[] {
	return value.split(/[\s,]+/).filter(Boolean);
}

function validKey(value: string, max: number) {
	return value.length <= max && keyPattern.test(value);
}

function validText(value: string, limit: number) {
	const trimmed = value.trim();
	return !!trimmed && encoder.encode(trimmed).length <= limit && !control.test(trimmed);
}

function cleanForm(value: string | null) {
	return value && validKey(value, 64) ? value : defaultFormKey;
}

export function defaultUiViewDraft(origin: CreatorOrigin): UiViewDraft {
	const formKey = cleanForm(origin.formKey);
	return {
		key: 'custom_data_view',
		name: 'Custom data view',
		publisher: 'Project author',
		version: '1.0.0',
		contributions: uiViewSlots.map((slot) => ({
			slot,
			enabled: slot === origin.slot,
			label: defaultLabels[slot],
			route: 'overview',
			formKey
		})),
		readFormKey: formKey,
		readFields: 'quantity, unit_price, amount',
		presentation: 'table',
		title: 'Data overview',
		text: 'Shows the fields this app has been granted from the selected Form.'
	};
}

export function invalidUiViewFields(draft: UiViewDraft): string[] {
	const bad: string[] = [];
	if (!validKey(draft.key.trim(), 64) || draft.key.trim() === 'library') bad.push('key');
	if (!validText(draft.name, 120)) bad.push('name');
	if (!validText(draft.publisher, 120)) bad.push('publisher');
	if (!releasePattern.test(draft.version.trim())) bad.push('version');
	if (!draft.contributions.some((item) => item.enabled)) bad.push('contributions');
	for (const item of draft.contributions.filter((entry) => entry.enabled)) {
		if (!validText(item.label, 120)) bad.push(`contributions.${item.slot}.label`);
		if (item.slot === 'project.view') {
			if (!validKey(item.route.trim(), 32)) bad.push(`contributions.${item.slot}.route`);
		} else if (!validKey(item.formKey.trim(), 64)) bad.push(`contributions.${item.slot}.formKey`);
	}
	if (!validKey(draft.readFormKey.trim(), 64)) bad.push('readFormKey');
	const fields = parseFields(draft.readFields);
	if (
		fields.length < 1 ||
		fields.length > 32 ||
		new Set(fields).size !== fields.length ||
		!fields.every((field) => validKey(field, 64))
	)
		bad.push('readFields');
	if (draft.presentation !== 'table' && draft.presentation !== 'summary') bad.push('presentation');
	if (!validText(draft.title, 120)) bad.push('title');
	if (!validText(draft.text, 320)) bad.push('text');
	return bad;
}

export function readUiViewDraft(stored: string | null, origin: CreatorOrigin): UiViewDraft {
	const defaults = defaultUiViewDraft(origin);
	if (!stored) return defaults;
	try {
		const prior: unknown = JSON.parse(stored);
		if (!prior || typeof prior !== 'object') return defaults;
		const record = prior as Record<string, unknown>;
		const text = (field: string, fallback: string) => {
			const value = record[field];
			return typeof value === 'string' && value.length <= 320 ? value : fallback;
		};
		const saved = Array.isArray(record.contributions) ? record.contributions : [];
		return {
			key: text('key', defaults.key),
			name: text('name', defaults.name),
			publisher: text('publisher', defaults.publisher),
			version: text('version', defaults.version),
			contributions: defaults.contributions.map((fallback) => {
				const item = saved.find(
					(entry) => entry && typeof entry === 'object' && entry.slot === fallback.slot
				) as Record<string, unknown> | undefined;
				if (!item) return fallback;
				const field = (name: 'label' | 'route' | 'formKey') =>
					typeof item[name] === 'string' && item[name].length <= 320
						? (item[name] as string)
						: fallback[name];
				return {
					slot: fallback.slot,
					enabled: typeof item.enabled === 'boolean' ? item.enabled : fallback.enabled,
					label: field('label'),
					route: field('route'),
					formKey: field('formKey')
				};
			}),
			readFormKey: text('readFormKey', defaults.readFormKey),
			readFields: text('readFields', defaults.readFields),
			presentation: record.presentation === 'summary' ? 'summary' : 'table',
			title: text('title', defaults.title),
			text: text('text', defaults.text)
		};
	} catch {
		return defaults;
	}
}

export function uiViewContributions(draft: UiViewDraft): PluginContributionV3[] {
	return uiViewSlots.flatMap((slot) => {
		const item = draft.contributions.find((entry) => entry.slot === slot);
		if (!item?.enabled) return [];
		const meta = slotMeta[slot];
		return [
			{
				id: meta.id,
				kind: meta.kind,
				slot,
				label: item.label.trim(),
				view: meta.view,
				route: slot === 'project.view' ? item.route.trim() : null,
				form_key: slot === 'project.view' ? null : item.formKey.trim()
			}
		];
	});
}

export function uiViewReads(draft: UiViewDraft): PluginReadV3[] {
	return [{ id: readId, form_key: draft.readFormKey.trim(), fields: parseFields(draft.readFields) }];
}

/** Injects the author's copy as inert JSON. `<` is escaped so the text can never close the script. */
export function renderUiViewHtml(draft: UiViewDraft): string {
	const copy = JSON.stringify({
		title: draft.title.trim(),
		text: draft.text.trim(),
		presentation: draft.presentation,
		read: readId
	}).replaceAll('<', '\\u003c');
	return html.replace(
		'<!--author-copy-->',
		() => '<script type="application/json" id="author-copy">' + copy + '</script>'
	);
}

function toBase64(bytes: Uint8Array): string {
	let binary = '';
	for (let index = 0; index < bytes.length; index += 8192)
		binary += String.fromCharCode(...bytes.subarray(index, index + 8192));
	return btoa(binary);
}

export async function buildUiViewPackage(
	draft: UiViewDraft
): Promise<{ upload: PluginUploadV3; html: string; json: string }> {
	if (invalidUiViewFields(draft).length) throw new Error('Invalid draft');
	const page = renderUiViewHtml(draft);
	const web = encoder.encode(page);
	if (web.length > maxHtmlBytes) throw new Error('Package page is too large');
	const manifest: PluginManifestV3 = {
		schema_version: 'openpr.plugin.v3',
		key: draft.key.trim(),
		name: draft.name.trim(),
		version: draft.version.trim(),
		publisher: draft.publisher.trim(),
		host_protocol: { min: 3, max: 3 },
		profile: 'ui',
		bridge_version: 2,
		entry: { path: 'panel.html', size: web.length, sha256: await sha256(web) },
		wasm: null,
		contributions: uiViewContributions(draft),
		reads: uiViewReads(draft),
		capabilities: []
	};
	const upload: PluginUploadV3 = { manifest, web_base64: toBase64(web) };
	return { upload, html: page, json: JSON.stringify(upload, null, 2) + '\n' };
}

export const uiViewTemplate: CreatorTemplate<UiViewDraft> = {
	id: 'ui_view',
	profile: 'ui',
	output: 'openpr.plugin.v3',
	slots: uiViewSlots,
	defaults: defaultUiViewDraft,
	read: readUiViewDraft,
	invalid: invalidUiViewFields,
	build: buildUiViewPackage,
	review: (draft) => ({
		slots: uiViewContributions(draft),
		reads: uiViewReads(draft),
		capabilities: []
	})
};

export interface SampleField {
	key: string;
	label: string;
	type: string;
	options?: string[];
}

const decimalTypes = ['number', 'integer', 'amount', 'rating', 'progress'];

/**
 * Sample rows for the isolated preview. Project previews follow the target Form's field types;
 * personal previews have no Form to read, so every value is the same placeholder.
 */
export function sampleRows(fields: SampleField[], personal: boolean) {
	return [1, 2, 3].map((n) => ({
		id: `00000000-0000-4000-8000-00000000000${n}`,
		values: Object.fromEntries(
			fields.map((field) => [field.key, personal ? '示例' : sampleValue(field, n)])
		),
		created_at: `2026-01-0${n}T00:00:00Z`,
		updated_at: `2026-01-0${n}T00:00:00Z`
	}));
}

function sampleValue(field: SampleField, n: number): unknown {
	if (decimalTypes.includes(field.type)) return { type: 'decimal', decimal: String(n * 10) };
	switch (field.type) {
		case 'boolean':
			return n % 2 === 1;
		case 'date':
			return `2026-01-0${n}`;
		case 'datetime':
			return `2026-01-0${n}T09:00:00Z`;
		case 'single_select':
			return field.options?.[0] ?? `Option ${n}`;
		case 'multi_select':
			return [field.options?.[0] ?? `Option ${n}`];
		default:
			return `Sample ${n}`;
	}
}

/** Selects the declared fields from the Form schema; undeclared Form fields are never shown. */
export function previewFields(read: PluginReadV3, schema: SampleField[] | null): SampleField[] {
	return read.fields.map(
		(key) => schema?.find((field) => field.key === key) ?? { key, label: key, type: 'text' }
	);
}

export function createMockTransport(
	read: PluginReadV3,
	schema: SampleField[] | null,
	personal: boolean
): PreviewTransport {
	const fields = previewFields(read, personal ? null : schema);
	const rows = sampleRows(fields, personal);
	return async (method, params) => {
		if (params.read !== read.id && method.startsWith('data.')) return { ok: false, code: 'denied' };
		if (method === 'data.describe')
			return {
				ok: true,
				result: {
					read: read.id,
					form: { key: read.form_key, name: read.form_key, fields },
					missing_fields: [],
					denied_fields: []
				}
			};
		if (method === 'data.query')
			return { ok: true, result: { read: read.id, items: rows, next_cursor: null } };
		if (method === 'data.record') {
			const item = rows.find((row) => row.id === params.record_id);
			return item ? { ok: true, result: { read: read.id, item } } : { ok: false, code: 'not_found' };
		}
		return { ok: false, code: 'method_unavailable' };
	};
}
