import {
	pluginCapabilities,
	type PluginInvocation,
	type PluginPanel
} from '../api/plugin-packages';

export const bridgeProtocol = 1;
const uuid = /^[a-f\d]{8}-[a-f\d]{4}-4[a-f\d]{3}-[89ab][a-f\d]{3}-[a-f\d]{12}$/i;

function isObject(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function pluginRequest(value: unknown) {
	if (!isObject(value) || value.protocol !== bridgeProtocol || value.type !== 'request') return;
	if (typeof value.request_id !== 'string' || !uuid.test(value.request_id)) return;
	const method = pluginCapabilities.find((capability) => capability === value.method);
	const validInput = isObject(value.input) && Object.keys(value.input).length === 0;
	return { method: validInput ? method : undefined, requestId: value.request_id };
}

export function isBridgeSignal(value: unknown, type: string): boolean {
	return isObject(value) && value.type === type && value.protocol === bridgeProtocol;
}

export async function verifyPanelHtml(
	panel: Pick<PluginPanel, 'html' | 'web_sha256'>
): Promise<boolean> {
	const bytes = new TextEncoder().encode(panel.html);
	const digest = await crypto.subtle.digest('SHA-256', bytes);
	const actual = [...new Uint8Array(digest)]
		.map((byte) => byte.toString(16).padStart(2, '0'))
		.join('');
	return actual === panel.web_sha256;
}

export function samePanel(first: PluginPanel, second: PluginPanel): boolean {
	return (
		first.package_id === second.package_id &&
		first.generation === second.generation &&
		first.version === second.version &&
		first.package_sha256 === second.package_sha256 &&
		first.web_sha256 === second.web_sha256 &&
		first.grants.length === second.grants.length &&
		first.grants.every((grant) => second.grants.includes(grant))
	);
}

export function validResult(
	result: PluginInvocation,
	panel: PluginPanel,
	requestId: string
): boolean {
	return (
		result.request_id === requestId &&
		result.package_id === panel.package_id &&
		result.generation === panel.generation &&
		result.version === panel.version &&
		Number.isSafeInteger(result.amount_cents) &&
		result.amount_cents >= 0 &&
		result.amount_cents <= 1_000_000_000_000
	);
}
