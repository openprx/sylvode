import type {
	PluginCapability,
	PluginContributionV3,
	PluginManifestV3,
	PluginReadV3,
	PluginSlotV3,
	PluginUpload,
	PluginUploadV3
} from '../../api/plugin-packages';
import { orderAmountTemplate, type CreatorDraft } from './template';
import { uiViewTemplate, type SampleField, type UiViewDraft } from './ui-view-template';

export type CreatorTemplateId = 'order_amount' | 'ui_view';
export interface CreatorOrigin {
	slot: PluginSlotV3;
	formKey: string | null;
}

export interface CreatorTemplate<Draft> {
	id: CreatorTemplateId;
	profile: 'wasm' | 'ui';
	output: 'openpr.plugin.v2' | 'openpr.plugin.v3';
	slots: PluginSlotV3[];
	defaults(origin: CreatorOrigin): Draft;
	read(stored: string | null, origin: CreatorOrigin): Draft;
	invalid(draft: Draft): string[];
	build(
		draft: Draft
	): Promise<{ upload: PluginUpload | PluginUploadV3; html: string; json: string }>;
	review(draft: Draft): {
		slots: PluginContributionV3[];
		reads: PluginReadV3[];
		capabilities: PluginCapability[];
	};
}

/** Transport shape accepted by the shared plugin frame host. */
export type PreviewTransport = (
	method: string,
	params: Record<string, unknown>,
	requestId: string
) => Promise<{ ok: true; result: unknown } | { ok: false; code: string }>;

/** What the isolated v3 preview needs: the package, the entry to show and where sample data comes from. */
export interface V3PreviewSpec {
	manifest: PluginManifestV3;
	contributionId: string;
	webSha256: string;
	/** Form fields for project previews; null falls back to text samples. */
	schema: SampleField[] | null;
	personal: boolean;
	onViewed: () => void;
	/** Answers `amount.preview` for packages that declare it; other previews cannot call capabilities. */
	onAmountPreview?: () => Promise<number | null>;
}

export const allSlots: PluginSlotV3[] = [
	'project.view',
	'form.list.toolbar',
	'form.record.detail',
	'form.record.actions'
];

export function originSlot(value: string | null): PluginSlotV3 {
	return allSlots.find((slot) => slot === value) ?? 'form.record.detail';
}

export function isCreatorTemplateId(value: unknown): value is CreatorTemplateId {
	return value === 'order_amount' || value === 'ui_view';
}

/** Which template a Creator session starts with for a launch slot and an optional explicit id. */
export function initialTemplate(explicit: string | null, slot: PluginSlotV3): CreatorTemplateId {
	if (isCreatorTemplateId(explicit)) return explicit;
	if (slot === 'project.view' || slot === 'form.list.toolbar') return 'ui_view';
	return 'order_amount';
}

export const creatorTemplates: {
	order_amount: CreatorTemplate<CreatorDraft>;
	ui_view: CreatorTemplate<UiViewDraft>;
} = { order_amount: orderAmountTemplate, ui_view: uiViewTemplate };

export function draftStorageKey(
	scope: 'personal' | 'project',
	actor: string,
	templateId: CreatorTemplateId,
	draftId: string
): string {
	return `sv.plugin.creator.v2:${scope}:${encodeURIComponent(actor)}:${templateId}:${draftId}`;
}

/** The v1 key an order amount draft was stored under before templates existed. */
export function legacyDraftKey(
	scope: 'personal' | 'project',
	actor: string,
	draftId: string
): string {
	return scope === 'personal'
		? `sv.plugin.creator.v1:personal:${encodeURIComponent(actor)}:${draftId}`
		: `sv.plugin.creator.v1:${actor}:${draftId}`;
}

export const KEY_PATTERN = /^[a-z][a-z0-9_]*$/;
const SEMVER =
	/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
const kinds: Record<PluginSlotV3, PluginContributionV3['kind']> = {
	'form.record.detail': 'panel',
	'form.record.actions': 'action',
	'form.list.toolbar': 'action',
	'project.view': 'view'
};
const encoder = new TextEncoder();

export interface ManifestV3Issue {
	reason: string;
	index?: number;
}

function key(value: unknown, max: number): value is string {
	return typeof value === 'string' && value.length <= max && KEY_PATTERN.test(value);
}

function label(value: unknown): value is string {
	return (
		typeof value === 'string' &&
		value.length > 0 &&
		encoder.encode(value).length <= 120 &&
		value === value.trim() &&
		!/[\u0000-\u001f\u007f-\u009f]/.test(value)
	);
}

/**
 * Frontend mirror of the server rules M1-X1 for the `ui` profile, used to catch a bad draft before
 * install. The server stays authoritative.
 */
export function validateManifestV3(manifest: PluginManifestV3): ManifestV3Issue | null {
	if (
		manifest.schema_version !== 'openpr.plugin.v3' ||
		manifest.host_protocol?.min !== 3 ||
		manifest.host_protocol?.max !== 3 ||
		manifest.bridge_version !== 2
	)
		return { reason: 'manifest_protocol_unsupported' };
	if (
		!key(manifest.key, 64) ||
		manifest.key === 'library' ||
		!label(manifest.name) ||
		!label(manifest.publisher) ||
		typeof manifest.version !== 'string' ||
		manifest.version.length > 64 ||
		!SEMVER.test(manifest.version)
	)
		return { reason: 'manifest_field_invalid' };
	const entry = manifest.entry;
	if (
		entry?.path !== 'panel.html' ||
		!Number.isInteger(entry.size) ||
		entry.size < 1 ||
		entry.size > 65536 ||
		!/^[0-9a-f]{64}$/.test(entry.sha256)
	)
		return { reason: 'asset_invalid' };
	if (manifest.profile === 'ui') {
		if (manifest.wasm !== null || manifest.capabilities.length !== 0)
			return { reason: 'profile_mismatch' };
	} else if (manifest.profile !== 'wasm' || manifest.wasm === null) {
		return { reason: 'profile_mismatch' };
	}
	const list = manifest.contributions;
	if (!Array.isArray(list) || list.length < 1 || list.length > 8)
		return { reason: 'contributions_invalid' };
	const ids = new Set<string>();
	const routes = new Set<string>();
	const targets = new Set<string>();
	let views = 0;
	for (const [index, item] of list.entries()) {
		const bad = { reason: 'contributions_invalid', index };
		if (!key(item.id, 64) || ids.has(item.id)) return bad;
		ids.add(item.id);
		if (kinds[item.slot] !== item.kind || !label(item.label) || !key(item.view, 64)) return bad;
		let target: string;
		if (item.slot === 'project.view') {
			if (item.form_key !== null || !key(item.route, 32) || routes.has(item.route)) return bad;
			routes.add(item.route);
			if (++views > 2) return bad;
			target = `${item.slot}:${item.route}`;
		} else {
			if (item.route !== null || !key(item.form_key, 64)) return bad;
			target = `${item.slot}:${item.form_key}`;
		}
		if (targets.has(target)) return bad;
		targets.add(target);
	}
	if (!Array.isArray(manifest.reads) || manifest.reads.length > 4)
		return { reason: 'reads_invalid' };
	const readIds = new Set<string>();
	for (const read of manifest.reads) {
		if (
			!key(read.id, 32) ||
			readIds.has(read.id) ||
			!key(read.form_key, 64) ||
			!Array.isArray(read.fields) ||
			read.fields.length < 1 ||
			read.fields.length > 32 ||
			new Set(read.fields).size !== read.fields.length ||
			!read.fields.every((field) => key(field, 64))
		)
			return { reason: 'reads_invalid' };
		readIds.add(read.id);
	}
	return null;
}
