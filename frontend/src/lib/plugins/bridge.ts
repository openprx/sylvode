import type { PluginKind, PluginSlotV3 } from '../api/plugin-packages';

/** Protocol of the shell handshake on window.postMessage. The port traffic uses `bridgeProtocolV2`. */
export const shellProtocol = 1;
const uuid = /^[a-f\d]{8}-[a-f\d]{4}-4[a-f\d]{3}-[89ab][a-f\d]{3}-[a-f\d]{12}$/i;

function isObject(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

export function isShellSignal(value: unknown, type: string): boolean {
	return isObject(value) && value.type === type && value.protocol === shellProtocol;
}

export async function verifyPanelHtml(panel: {
	html: string;
	web_sha256: string;
}): Promise<boolean> {
	const bytes = new TextEncoder().encode(panel.html);
	const digest = await crypto.subtle.digest('SHA-256', bytes);
	const actual = [...new Uint8Array(digest)]
		.map((byte) => byte.toString(16).padStart(2, '0'))
		.join('');
	return actual === panel.web_sha256;
}

/* ------------------------------------------------------------------------------------------
 * Bridge protocol 2 (manifest v3, the only bridge). The shell handshake on window.postMessage
 * uses `shellProtocol`; the MessagePort traffic between the plugin and the host uses protocol 2.
 * ---------------------------------------------------------------------------------------- */

export const bridgeProtocolV2 = 2;

/** Port methods a protocol 2 plugin may request. Anything else is `method_unavailable`. */
export const bridgeMethods = [
	'host.context',
	'data.describe',
	'data.query',
	'data.record',
	'host.navigate',
	'amount.preview',
	'amount.apply'
] as const;
export type BridgeMethod = (typeof bridgeMethods)[number];

export const bridgeErrorCodes = [
	'method_unavailable',
	'invalid_params',
	'busy',
	'denied',
	'stale',
	'not_found',
	'rate_limited',
	'timeout',
	'canceled',
	'preview_required',
	'failed'
] as const;
export type BridgeErrorCode = (typeof bridgeErrorCodes)[number];

export function bridgeErrorCode(value: unknown): BridgeErrorCode {
	return bridgeErrorCodes.find((code) => code === value) ?? 'failed';
}

/** Per-frame limits from the protocol 2 contract. */
export const bridgeLimits = {
	requestsPerFrame: 500,
	dataInFlight: 4,
	stateLength: 512
} as const;

/** Display context handed to protocol 2 plugins. Never carries project or user IDs. */
export interface PluginHostContext {
	package: { key: string; version: string };
	contribution: { id: string; kind: PluginKind; slot: PluginSlotV3; view: string };
	project: { name: string };
	form: { key: string } | null;
	record: { id: string } | null;
	route: { state: string | null } | null;
	locale: 'zh' | 'en';
	theme: 'light' | 'dark';
}

export type BridgeV2Message =
	| { type: 'ready' }
	| { type: 'close' }
	| { type: 'resize'; height: number }
	| {
			type: 'request';
			requestId: string;
			/** Absent when the method is not on the protocol 2 allow-list. */
			method?: BridgeMethod;
			/** Absent when `params` is not a plain object. */
			params?: Record<string, unknown>;
	  };

/**
 * Parses one protocol 2 port message. Returns `undefined` for anything that cannot be
 * answered (wrong protocol, unknown type, missing or non-v4 request ID, non-integer height).
 */
export function parseBridgeV2(value: unknown): BridgeV2Message | undefined {
	if (!isObject(value) || value.protocol !== bridgeProtocolV2) return;
	switch (value.type) {
		case 'ready':
			return { type: 'ready' };
		case 'close':
			return { type: 'close' };
		case 'resize':
			return Number.isSafeInteger(value.height)
				? { type: 'resize', height: value.height as number }
				: undefined;
		case 'request': {
			if (typeof value.request_id !== 'string' || !uuid.test(value.request_id)) return;
			const method = bridgeMethods.find((name) => name === value.method);
			return {
				type: 'request',
				requestId: value.request_id,
				...(method ? { method } : {}),
				...(isObject(value.params) ? { params: value.params } : {})
			};
		}
		default:
			return;
	}
}

/** Clamps a plugin-requested frame height into the host's bounds. */
export function clampFrameHeight(height: number, min: number, max: number): number {
	const low = Math.min(min, max);
	return Math.min(Math.max(Math.round(height), low), max);
}

/** Builds the shell init message: the plugin page, its grants and the display context. */
export function shellInitMessage(init: {
	html: string;
	grants: string[];
	context: PluginHostContext;
}) {
	return {
		protocol: shellProtocol,
		type: 'sylvode-plugin-shell-init',
		bridge: bridgeProtocolV2,
		html: init.html,
		grants: init.grants,
		context: init.context
	};
}
