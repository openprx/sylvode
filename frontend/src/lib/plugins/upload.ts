import type { PluginUpload } from '../api/plugin-packages';
import { isContribution } from './placements';

export async function readPluginUpload(file: File): Promise<PluginUpload> {
	if (file.size > 8 * 1024 * 1024) throw new Error();
	const value = JSON.parse(await file.text()) as PluginUpload;
	if (
		!value ||
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
