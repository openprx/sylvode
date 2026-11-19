import type { ApiResult } from '../api/client';
import {
	pluginExtensionsApi,
	pluginPackagesApi,
	type PluginCapability,
	type PluginContributionItem,
	type PluginDataRequest,
	type PluginInvocation,
	type PluginPanel
} from '../api/plugin-packages';
import {
	bridgeErrorCode,
	bridgeLimits,
	validResult,
	type BridgeErrorCode,
	type BridgeMethod,
	type PluginHostContext
} from './bridge';

/** What a host transport resolves to. `code` is a bridge error code; unknown codes become `failed`. */
export type HostTransportResult = { ok: true; result: unknown } | { ok: false; code: string };

/**
 * Performs one already-authorised host method. The dispatcher only calls it after the method,
 * the grant, the params and the in-flight limits have been checked. For `amount.apply` the
 * dispatcher passes `{ preview_request_id }` itself; plugins can never supply it.
 */
export type HostTransport = (
	method: string,
	params: Record<string, unknown>,
	requestId: string
) => Promise<HostTransportResult>;

/**
 * Result of dispatching one request. `close: 'suspend'` means access was withdrawn (send
 * `suspended`, then close); `close: 'timeout'` means an `amount.*` call outlived the host
 * timeout and may have committed, so the frame must close and the record be reloaded.
 */
export type HostOutcome =
	| { ok: true; result: unknown }
	| { ok: false; code: BridgeErrorCode; close?: 'suspend' | 'timeout' };

export type NavigateTarget =
	| { target: 'view'; route: string; state: string | null }
	| { target: 'record'; record_id: string }
	| { target: 'project' };

const keyPattern = /^[a-z][a-z0-9_]*$/;
const anyUuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const cursorLength = 2048;

function isObject(value: unknown): value is Record<string, unknown> {
	return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function onlyKeys(params: Record<string, unknown>, allowed: readonly string[]): boolean {
	return Object.keys(params).every((key) => allowed.includes(key));
}

function validKey(value: unknown, max: number): value is string {
	return typeof value === 'string' && value.length <= max && keyPattern.test(value);
}

const isDataMethod = (method: BridgeMethod) => method.startsWith('data.');
const isAmountMethod = (method: string): method is PluginCapability =>
	method === 'amount.preview' || method === 'amount.apply';

/** Strictly validates `host.navigate` params. Arbitrary URLs never match any shape. */
export function navigateTarget(params: Record<string, unknown>): NavigateTarget | null {
	switch (params.target) {
		case 'view': {
			if (!onlyKeys(params, ['target', 'route', 'state'])) return null;
			if (!validKey(params.route, 32)) return null;
			const state = params.state ?? null;
			if (state !== null && (typeof state !== 'string' || state.length > bridgeLimits.stateLength))
				return null;
			return { target: 'view', route: params.route, state };
		}
		case 'record':
			if (!onlyKeys(params, ['target', 'record_id'])) return null;
			if (typeof params.record_id !== 'string' || !anyUuid.test(params.record_id)) return null;
			return { target: 'record', record_id: params.record_id.toLowerCase() };
		case 'project':
			return onlyKeys(params, ['target']) ? { target: 'project' } : null;
		default:
			return null;
	}
}

/**
 * Validates and normalises the params of an allow-listed method. Returns `null` when they are
 * malformed (the request is answered with `invalid_params`).
 */
export function hostMethodParams(
	method: BridgeMethod,
	params: Record<string, unknown>
): Record<string, unknown> | null {
	switch (method) {
		case 'host.context':
		case 'amount.preview':
		case 'amount.apply':
			return Object.keys(params).length === 0 ? {} : null;
		case 'data.describe':
			return onlyKeys(params, ['read']) && validKey(params.read, 32) ? { read: params.read } : null;
		case 'data.query': {
			if (!onlyKeys(params, ['read', 'limit', 'cursor']) || !validKey(params.read, 32)) return null;
			const limit = params.limit ?? null;
			const cursor = params.cursor ?? null;
			if (
				limit !== null &&
				(!Number.isSafeInteger(limit) || (limit as number) < 1 || (limit as number) > 50)
			)
				return null;
			if (
				cursor !== null &&
				(typeof cursor !== 'string' || cursor.length === 0 || cursor.length > cursorLength)
			)
				return null;
			return { read: params.read, ...(limit === null ? {} : { limit }), cursor };
		}
		case 'data.record':
			if (!onlyKeys(params, ['read', 'record_id']) || !validKey(params.read, 32)) return null;
			if (typeof params.record_id !== 'string' || !anyUuid.test(params.record_id)) return null;
			return { read: params.read, record_id: params.record_id.toLowerCase() };
		case 'host.navigate': {
			const target = navigateTarget(params);
			return target ? { ...target } : null;
		}
	}
}

/** The grant a method needs, given its validated params. */
export function requiredGrant(method: BridgeMethod, params: Record<string, unknown>): string {
	if (isDataMethod(method)) return `read:${String(params.read)}`;
	if (isAmountMethod(method)) return method;
	return 'ui.display';
}

export interface HostDispatcherOptions {
	bridge: 1 | 2;
	/** The frame's effective grants (from discovery or entry; never from the plugin). */
	grants: () => readonly string[];
	context: () => PluginHostContext | undefined;
	transport: HostTransport;
	/** Host confirmation before `amount.apply`. Missing means apply is always canceled. */
	confirmApply?: () => Promise<boolean>;
	timeoutMs?: number;
}

export interface HostDispatcher {
	handle(
		requestId: string,
		method: BridgeMethod | undefined,
		params: Record<string, unknown> | undefined
	): Promise<HostOutcome>;
	dispose(): void;
}

const fail = (code: BridgeErrorCode): HostOutcome => ({ ok: false, code });

/**
 * One dispatcher per frame. Enforces the allow-list, grants, params, request ID de-duplication,
 * the per-frame request cap, the data in-flight cap and `amount.*` serialisation.
 */
export function createHostDispatcher(options: HostDispatcherOptions): HostDispatcher {
	const timeoutMs = options.timeoutMs ?? 10_000;
	const seen = new Set<string>();
	let dataInFlight = 0;
	let navigating = false;
	let amountBusy = false;
	let preview: string | null = null;
	let disposed = false;

	async function call(
		method: string,
		params: Record<string, unknown>,
		requestId: string
	): Promise<HostOutcome> {
		let timer: ReturnType<typeof setTimeout> | undefined;
		const expired = new Promise<'timeout'>((resolve) => {
			timer = setTimeout(() => resolve('timeout'), timeoutMs);
		});
		const pending = Promise.resolve()
			.then(() => options.transport(method, params, requestId))
			.then(
				(value): HostOutcome => {
					if (isObject(value) && value.ok === true) return { ok: true, result: value.result };
					const code = bridgeErrorCode(isObject(value) ? value.code : undefined);
					return code === 'denied' ? { ok: false, code, close: 'suspend' } : fail(code);
				},
				(): HostOutcome => fail('failed')
			);
		const outcome = await Promise.race([pending, expired]);
		clearTimeout(timer);
		if (outcome === 'timeout')
			return isAmountMethod(method)
				? { ok: false, code: 'timeout', close: 'timeout' }
				: fail('timeout');
		return outcome;
	}

	async function amount(method: PluginCapability, requestId: string): Promise<HostOutcome> {
		amountBusy = true;
		try {
			if (method === 'amount.preview') {
				preview = null;
				const outcome = await call(method, {}, requestId);
				if (outcome.ok && !disposed) preview = requestId;
				return outcome;
			}
			const authorized = preview;
			if (!authorized) return fail('preview_required');
			let confirmed = false;
			try {
				confirmed = options.confirmApply ? await options.confirmApply() : false;
			} catch {
				confirmed = false;
			}
			if (disposed || !confirmed) return fail('canceled');
			const outcome = await call(method, { preview_request_id: authorized }, requestId);
			preview = null;
			return outcome;
		} finally {
			amountBusy = false;
		}
	}

	async function handleLegacy(
		requestId: string,
		method: BridgeMethod | undefined
	): Promise<HostOutcome> {
		if (!method || !isAmountMethod(method) || !options.grants().includes(method))
			return fail('method_unavailable');
		if (amountBusy || seen.has(requestId) || seen.size >= bridgeLimits.legacyRequestsPerFrame)
			return fail('busy');
		seen.add(requestId);
		return amount(method, requestId);
	}

	async function handleV2(
		requestId: string,
		method: BridgeMethod | undefined,
		raw: Record<string, unknown> | undefined
	): Promise<HostOutcome> {
		if (seen.has(requestId)) return fail('invalid_params');
		if (seen.size >= bridgeLimits.requestsPerFrame) return fail('busy');
		seen.add(requestId);
		if (!method) return fail('method_unavailable');
		const grants = options.grants();
		const reachable = isDataMethod(method)
			? grants.some((grant) => grant.startsWith('read:'))
			: grants.includes(requiredGrant(method, {}));
		if (!reachable) return fail('method_unavailable');
		const params = raw ? hostMethodParams(method, raw) : null;
		if (!params) return fail('invalid_params');
		if (!grants.includes(requiredGrant(method, params))) return fail('method_unavailable');

		if (method === 'host.context') {
			const context = options.context();
			return context ? { ok: true, result: context } : fail('failed');
		}
		if (isAmountMethod(method)) return amountBusy ? fail('busy') : amount(method, requestId);
		if (method === 'host.navigate') {
			if (navigating) return fail('busy');
			navigating = true;
			try {
				return await call(method, params, requestId);
			} finally {
				navigating = false;
			}
		}
		if (dataInFlight >= bridgeLimits.dataInFlight) return fail('busy');
		dataInFlight++;
		try {
			return await call(method, params, requestId);
		} finally {
			dataInFlight--;
		}
	}

	return {
		async handle(requestId, method, params) {
			if (disposed) return fail('canceled');
			const outcome =
				options.bridge === 1
					? await handleLegacy(requestId, method)
					: await handleV2(requestId, method, params);
			return disposed ? fail('canceled') : outcome;
		},
		dispose() {
			disposed = true;
			preview = null;
		}
	};
}

/** Maps an API failure envelope to a bridge error code (design §2.6.2). */
export function mapApiFailure(
	response: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): BridgeErrorCode {
	const reason = isObject(response.details) ? response.details.reason : undefined;
	if (response.error_code === 'version_conflict') return 'stale';
	if (response.code === 401 || response.code === 403 || reason === 'contribution_unavailable')
		return 'denied';
	if (reason === 'record_unavailable') return 'not_found';
	if (reason === 'data_rate_limited') return 'rate_limited';
	return 'failed';
}

const dataOps = {
	'data.describe': 'form.describe',
	'data.query': 'records.query',
	'data.record': 'record.get'
} as const;

function dataRequest(
	method: keyof typeof dataOps,
	params: Record<string, unknown>
): PluginDataRequest {
	const read = String(params.read);
	if (method === 'data.describe') return { op: dataOps[method], read };
	if (method === 'data.record')
		return { op: dataOps[method], read, record_id: String(params.record_id) };
	return {
		op: dataOps[method],
		read,
		...(typeof params.limit === 'number' ? { limit: params.limit } : {}),
		cursor: typeof params.cursor === 'string' ? params.cursor : null
	};
}

function validAmount(result: PluginInvocation): boolean {
	return (
		Number.isSafeInteger(result.amount_cents) &&
		result.amount_cents >= 0 &&
		result.amount_cents <= 1_000_000_000_000
	);
}

export interface ExtensionTransportOptions {
	projectId: string;
	/** From the entry response. `package_id` and `generation` are sent on every call. */
	item: Pick<
		PluginContributionItem,
		'package_id' | 'key' | 'version' | 'generation' | 'contribution'
	>;
	/** Required for record contributions; `amount.*` is unavailable without it. */
	recordId?: string;
	/** Routes of this package's currently enabled `project.view` contributions. */
	enabledRoutes: () => readonly string[];
	navigate: (target: NavigateTarget) => void | Promise<void>;
	/** Called on `stale` so the caller can rediscover. */
	onStale?: () => void;
	/** Called after a successful `amount.apply`. */
	onApplied?: () => void;
	api?: Pick<typeof pluginExtensionsApi, 'data' | 'invoke'>;
}

/** Transport for protocol 2 frames backed by the v3 data, invoke and routing endpoints. */
export function createExtensionTransport(options: ExtensionTransportOptions): HostTransport {
	const api = options.api ?? pluginExtensionsApi;
	const { item } = options;
	const failure = (response: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>) => {
		const code = mapApiFailure(response);
		if (code === 'stale') options.onStale?.();
		return { ok: false as const, code };
	};

	return async (method, params, requestId) => {
		if (method === 'data.describe' || method === 'data.query' || method === 'data.record') {
			const response = await api.data(options.projectId, item.key, item.contribution.id, {
				...dataRequest(method, params),
				package_id: item.package_id,
				generation: item.generation,
				request_id: requestId
			});
			return response.code === 0 ? { ok: true, result: response.data } : failure(response);
		}
		if (method === 'host.navigate') {
			const target = navigateTarget(params);
			if (!target) return { ok: false, code: 'invalid_params' };
			if (target.target === 'view' && !options.enabledRoutes().includes(target.route))
				return { ok: false, code: 'invalid_params' };
			await options.navigate(target);
			return { ok: true, result: { navigated: true } };
		}
		if (isAmountMethod(method)) {
			const recordId = options.recordId;
			if (!recordId) return { ok: false, code: 'method_unavailable' };
			const previewRequestId =
				typeof params.preview_request_id === 'string' ? params.preview_request_id : undefined;
			if (method === 'amount.apply' && !previewRequestId)
				return { ok: false, code: 'preview_required' };
			const response = await api.invoke(recordId, item.key, item.contribution.id, {
				package_id: item.package_id,
				generation: item.generation,
				request_id: requestId,
				...(previewRequestId && method === 'amount.apply'
					? { preview_request_id: previewRequestId }
					: {}),
				capability: method,
				input: {}
			});
			if (response.code !== 0) return failure(response);
			const result = response.data;
			if (
				!result ||
				result.request_id !== requestId ||
				result.package_id !== item.package_id ||
				result.generation !== item.generation ||
				result.version !== item.version ||
				!validAmount(result)
			)
				return { ok: false, code: 'failed' };
			if (method === 'amount.apply') options.onApplied?.();
			return { ok: true, result: { amount_cents: result.amount_cents } };
		}
		return { ok: false, code: 'method_unavailable' };
	};
}

export interface LegacyTransportOptions {
	recordId: string;
	panel: PluginPanel;
	/** Called after a successful `amount.apply`. */
	onApplied?: () => void;
	api?: Pick<typeof pluginPackagesApi, 'invoke'>;
}

/** Transport for protocol 1 frames, matching the previous record panel host behaviour. */
export function createLegacyTransport(options: LegacyTransportOptions): HostTransport {
	const api = options.api ?? pluginPackagesApi;
	return async (method, params, requestId) => {
		if (!isAmountMethod(method)) return { ok: false, code: 'method_unavailable' };
		const previewRequestId =
			method === 'amount.apply' && typeof params.preview_request_id === 'string'
				? params.preview_request_id
				: undefined;
		if (method === 'amount.apply' && !previewRequestId)
			return { ok: false, code: 'preview_required' };
		const response = await api.invoke(
			options.recordId,
			options.panel,
			method,
			requestId,
			previewRequestId
		);
		if (
			response.code !== 0 ||
			!response.data ||
			!validResult(response.data, options.panel, requestId)
		)
			return {
				ok: false,
				code: [401, 403, 404].includes(response.code) ? 'denied' : 'failed'
			};
		if (method === 'amount.apply') options.onApplied?.();
		return { ok: true, result: { amount_cents: response.data.amount_cents } };
	};
}

/** Builds the minimal display context for a contribution (no project or user IDs). */
export function hostContext(
	item: Pick<PluginContributionItem, 'key' | 'version' | 'contribution'>,
	env: {
		projectName: string;
		recordId?: string | null;
		routeState?: string | null;
		locale: string;
		theme: 'light' | 'dark';
	}
): PluginHostContext {
	const { contribution } = item;
	const recordSlot =
		contribution.slot === 'form.record.detail' || contribution.slot === 'form.record.actions';
	return {
		package: { key: item.key, version: item.version },
		contribution: {
			id: contribution.id,
			kind: contribution.kind,
			slot: contribution.slot,
			view: contribution.view
		},
		project: { name: env.projectName },
		form: contribution.form_key ? { key: contribution.form_key } : null,
		record: recordSlot && env.recordId ? { id: env.recordId } : null,
		route: contribution.slot === 'project.view' ? { state: env.routeState ?? null } : null,
		locale: env.locale.toLowerCase().startsWith('zh') ? 'zh' : 'en',
		theme: env.theme
	};
}
