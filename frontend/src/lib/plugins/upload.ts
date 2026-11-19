import type { PluginUpload, PluginUploadV3 } from '../api/plugin-packages';
import { isContribution } from './placements';

const slots = ['form.record.detail', 'form.record.actions', 'form.list.toolbar', 'project.view'];
const kinds = ['panel', 'action', 'view'];

function isObject(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function isContributionV3(value: unknown): boolean {
	return (
		isObject(value) &&
		typeof value.id === 'string' &&
		typeof value.label === 'string' &&
		typeof value.view === 'string' &&
		kinds.includes(value.kind as string) &&
		slots.includes(value.slot as string) &&
		(value.route === null || typeof value.route === 'string') &&
		(value.form_key === null || typeof value.form_key === 'string')
	);
}

function isReadV3(value: unknown): boolean {
	return (
		isObject(value) &&
		typeof value.id === 'string' &&
		typeof value.form_key === 'string' &&
		Array.isArray(value.fields) &&
		value.fields.every((field) => typeof field === 'string')
	);
}

/** Structural check of a v3 upload. Rule validation stays on the server. */
function validV3(value: Record<string, unknown>): boolean {
	const manifest = value.manifest;
	if (!isObject(manifest) || 'ui' in manifest) return false;
	const entry = manifest.entry;
	return (
		typeof value.web_base64 === 'string' &&
		(value.wasm_base64 === undefined || typeof value.wasm_base64 === 'string') &&
		(manifest.profile === 'ui' || manifest.profile === 'wasm') &&
		(manifest.profile === 'ui'
			? value.wasm_base64 === undefined
			: typeof value.wasm_base64 === 'string') &&
		typeof manifest.key === 'string' &&
		typeof manifest.name === 'string' &&
		typeof manifest.version === 'string' &&
		typeof manifest.publisher === 'string' &&
		isObject(entry) &&
		typeof entry.sha256 === 'string' &&
		Array.isArray(manifest.contributions) &&
		manifest.contributions.every(isContributionV3) &&
		Array.isArray(manifest.reads) &&
		manifest.reads.every(isReadV3) &&
		Array.isArray(manifest.capabilities) &&
		manifest.capabilities.every((item) => isObject(item) && typeof item.name === 'string')
	);
}

export async function readPluginUpload(file: File): Promise<PluginUpload | PluginUploadV3> {
	if (file.size > 8 * 1024 * 1024) throw new Error();
	const parsed: unknown = JSON.parse(await file.text());
	if (!isObject(parsed)) throw new Error();
	if (isObject(parsed.manifest) && parsed.manifest.schema_version === 'openpr.plugin.v3') {
		if (!validV3(parsed)) throw new Error();
		return parsed as unknown as PluginUploadV3;
	}
	const value = parsed as unknown as PluginUpload;
	if (
		typeof value.wasm_base64 !== 'string' ||
		typeof value.web_base64 !== 'string' ||
		typeof value.manifest?.key !== 'string' ||
		typeof value.manifest?.name !== 'string' ||
		typeof value.manifest?.version !== 'string' ||
		typeof value.manifest?.publisher !== 'string' ||
		typeof value.manifest?.ui?.form_key !== 'string' ||
		typeof value.manifest?.ui?.bridge_version !== 'number' ||
		!Array.isArray(value.manifest?.capabilities) ||
		!value.manifest.capabilities.every((capability) => typeof capability?.name === 'string') ||
		(value.manifest.ui.contributions !== undefined &&
			(!Array.isArray(value.manifest.ui.contributions) ||
				!value.manifest.ui.contributions.every(isContribution)))
	)
		throw new Error();
	return value;
}
