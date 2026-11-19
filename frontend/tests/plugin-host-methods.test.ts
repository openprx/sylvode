import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { ApiResult } from '../src/lib/api/client';
import type {
	PluginContributionItem,
	PluginInvocation,
	PluginPanel
} from '../src/lib/api/plugin-packages';
import type { PluginHostContext } from '../src/lib/plugins/bridge';
import {
	createExtensionTransport,
	createHostDispatcher,
	createLegacyTransport,
	hostContext,
	hostMethodParams,
	mapApiFailure,
	navigateTarget,
	type HostTransport,
	type NavigateTarget
} from '../src/lib/plugins/host-methods';

const id = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;
const recordId = '11111111-2222-4333-8444-555555555555';
const ok = <T>(data: T): ApiResult<T> => ({ code: 0, message: 'ok', data });
const err = (code: number, error_code?: string, reason?: string): ApiResult<never> => ({
	code,
	message: 'x',
	data: null,
	...(error_code ? { error_code } : {}),
	...(reason ? { details: { reason } } : {})
});

const item: PluginContributionItem = {
	package_id: 'pkg-1',
	key: 'order_board',
	version: '1.0.0',
	name: 'Order board',
	profile: 'wasm',
	generation: 7,
	package_sha256: 'p',
	web_sha256: 'w',
	contribution: {
		id: 'summary',
		kind: 'panel',
		slot: 'form.record.detail',
		label: 'Summary',
		view: 'summary',
		route: null,
		form_key: 'order_line'
	},
	project_position: 50,
	preference: { hidden: false, collapsed: false, position: null },
	effective_position: 50,
	grants: ['ui.display', 'read:orders', 'amount.preview', 'amount.apply']
};

const context = hostContext(item, {
	projectName: 'Demo',
	recordId,
	routeState: 'ignored',
	locale: 'zh-CN',
	theme: 'dark'
});

function dispatcher(grants: string[], transport: HostTransport, confirm?: () => Promise<boolean>) {
	return createHostDispatcher({
		bridge: 2,
		grants: () => grants,
		context: () => context,
		transport,
		confirmApply: confirm,
		timeoutMs: 50
	});
}

const never: HostTransport = async () => {
	throw new Error('transport must not be called');
};

describe('host method authorisation', () => {
	test('missing grants return method_unavailable before reaching the transport', async () => {
		const host = dispatcher(['ui.display'], never);
		for (const [n, method, params] of [
			[1, 'data.query', { read: 'orders' }],
			[2, 'data.describe', { read: 'orders' }],
			[3, 'data.record', { read: 'orders', record_id: recordId }],
			[4, 'amount.preview', {}],
			[5, 'amount.apply', {}]
		] as const)
			assert.deepEqual(await host.handle(id(n), method, params), {
				ok: false,
				code: 'method_unavailable'
			});
		const reader = dispatcher(['ui.display', 'read:orders'], never);
		assert.deepEqual(await reader.handle(id(6), 'data.query', { read: 'customers' }), {
			ok: false,
			code: 'method_unavailable'
		});
		const none = dispatcher([], never);
		assert.deepEqual(await none.handle(id(7), 'host.context', {}), {
			ok: false,
			code: 'method_unavailable'
		});
		assert.deepEqual(await none.handle(id(8), undefined, {}), {
			ok: false,
			code: 'method_unavailable'
		});
	});

	test('malformed params return invalid_params', async () => {
		const host = dispatcher(['ui.display', 'read:orders'], never);
		assert.deepEqual(await host.handle(id(1), 'data.query', { read: 'orders', limit: 51 }), {
			ok: false,
			code: 'invalid_params'
		});
		assert.deepEqual(await host.handle(id(2), 'data.query', undefined), {
			ok: false,
			code: 'invalid_params'
		});
		assert.deepEqual(await host.handle(id(3), 'host.context', { x: 1 }), {
			ok: false,
			code: 'invalid_params'
		});
		assert.equal(hostMethodParams('data.record', { read: 'orders', record_id: 'nope' }), null);
		assert.equal(hostMethodParams('data.describe', { read: 'orders', project_id: 'x' }), null);
		assert.deepEqual(hostMethodParams('data.query', { read: 'orders', limit: null }), {
			read: 'orders',
			cursor: null
		});
	});

	test('the context carries no project or user IDs and follows slot rules', () => {
		assert.deepEqual(context, {
			package: { key: 'order_board', version: '1.0.0' },
			contribution: { id: 'summary', kind: 'panel', slot: 'form.record.detail', view: 'summary' },
			project: { name: 'Demo' },
			form: { key: 'order_line' },
			record: { id: recordId },
			route: null,
			locale: 'zh',
			theme: 'dark'
		} satisfies PluginHostContext);
	});
});

describe('host.navigate', () => {
	test('rejects external URLs and unknown shapes at the dispatcher', async () => {
		const host = dispatcher(['ui.display'], never);
		for (const [n, params] of [
			[1, { target: 'url', url: 'https://evil.example' }],
			[2, { target: 'view', route: 'https://evil.example/board' }],
			[3, { target: 'view', route: '../../admin' }],
			[4, { target: 'view', route: 'board', url: '//evil.example' }],
			[5, { target: 'view', route: 'board', state: 'x'.repeat(513) }],
			[6, { target: 'record', record_id: 'javascript:alert(1)' }],
			[7, { target: 'project', href: '/elsewhere' }]
		] as const)
			assert.deepEqual(await host.handle(id(n), 'host.navigate', params), {
				ok: false,
				code: 'invalid_params'
			});
	});

	test('rejects routes of other packages or disabled views at the transport', async () => {
		const visited: NavigateTarget[] = [];
		const transport = createExtensionTransport({
			projectId: 'p',
			item,
			enabledRoutes: () => ['board'],
			navigate: (target) => {
				visited.push(target);
			},
			api: { data: never as never, invoke: never as never }
		});
		const host = dispatcher(['ui.display'], transport);
		assert.deepEqual(
			await host.handle(id(1), 'host.navigate', { target: 'view', route: 'other' }),
			{
				ok: false,
				code: 'invalid_params'
			}
		);
		assert.deepEqual(
			await host.handle(id(2), 'host.navigate', { target: 'view', route: 'board', state: 's' }),
			{ ok: true, result: { navigated: true } }
		);
		assert.deepEqual(
			await host.handle(id(3), 'host.navigate', {
				target: 'record',
				record_id: recordId.toUpperCase()
			}),
			{ ok: true, result: { navigated: true } }
		);
		assert.deepEqual(visited, [
			{ target: 'view', route: 'board', state: 's' },
			{ target: 'record', record_id: recordId }
		]);
		assert.equal(navigateTarget({ target: 'project' })?.target, 'project');
	});
});

describe('HTTP error mapping', () => {
	test('maps envelopes to bridge codes', () => {
		assert.equal(
			mapApiFailure(err(409, 'version_conflict', 'form_plugin_binding_changed')),
			'stale'
		);
		assert.equal(mapApiFailure(err(401)), 'denied');
		assert.equal(mapApiFailure(err(403, 'forbidden', 'data_read_not_granted')), 'denied');
		assert.equal(mapApiFailure(err(404, 'not_found', 'contribution_unavailable')), 'denied');
		assert.equal(mapApiFailure(err(404, 'not_found', 'record_unavailable')), 'not_found');
		assert.equal(mapApiFailure(err(400, 'limit_exceeded', 'data_rate_limited')), 'rate_limited');
		assert.equal(mapApiFailure(err(409)), 'failed');
		assert.equal(mapApiFailure(err(500)), 'failed');
	});

	test('the data transport sends host-owned package and generation and suspends on denial', async () => {
		const bodies: unknown[] = [];
		const responses = [
			ok({ read: 'orders', items: [] }),
			err(409, 'version_conflict'),
			err(404, 'not_found', 'contribution_unavailable')
		];
		let stale = 0;
		const transport = createExtensionTransport({
			projectId: 'p',
			item,
			enabledRoutes: () => [],
			navigate: () => undefined,
			onStale: () => stale++,
			api: {
				data: async (_p, _k, _c, body) => {
					bodies.push(body);
					return responses.shift() as ApiResult<unknown>;
				},
				invoke: never as never
			}
		});
		const host = dispatcher(['ui.display', 'read:orders'], transport);
		assert.deepEqual(await host.handle(id(1), 'data.query', { read: 'orders', limit: 10 }), {
			ok: true,
			result: { read: 'orders', items: [] }
		});
		assert.deepEqual(await host.handle(id(2), 'data.describe', { read: 'orders' }), {
			ok: false,
			code: 'stale'
		});
		assert.equal(stale, 1);
		assert.deepEqual(await host.handle(id(3), 'data.describe', { read: 'orders' }), {
			ok: false,
			code: 'denied',
			close: 'suspend'
		});
		assert.deepEqual(bodies[0], {
			op: 'records.query',
			read: 'orders',
			limit: 10,
			cursor: null,
			package_id: 'pkg-1',
			generation: 7,
			request_id: id(1)
		});
	});

	test('timeouts and thrown transports never leak through', async () => {
		const host = dispatcher(['ui.display', 'read:orders', 'amount.preview'], async (method) => {
			if (method === 'data.describe') throw new Error('network');
			return new Promise(() => undefined);
		});
		assert.deepEqual(await host.handle(id(1), 'data.describe', { read: 'orders' }), {
			ok: false,
			code: 'failed'
		});
		assert.deepEqual(await host.handle(id(2), 'data.query', { read: 'orders' }), {
			ok: false,
			code: 'timeout'
		});
		assert.deepEqual(await host.handle(id(3), 'amount.preview', {}), {
			ok: false,
			code: 'timeout',
			close: 'timeout'
		});
	});

	test('unknown transport codes become failed', async () => {
		const host = dispatcher(['ui.display'], async () => ({ ok: false, code: 'teapot' }));
		assert.deepEqual(await host.handle(id(1), 'host.navigate', { target: 'project' }), {
			ok: false,
			code: 'failed'
		});
	});
});

describe('amount methods', () => {
	const invocation = (requestId: string, cents = 2500): PluginInvocation => ({
		request_id: requestId,
		package_id: 'pkg-1',
		version: '1.0.0',
		generation: 7,
		amount_cents: cents
	});

	test('apply needs a preview and host confirmation, and uses the host preview ID', async () => {
		const calls: unknown[] = [];
		let applied = 0;
		const transport = createExtensionTransport({
			projectId: 'p',
			item,
			recordId,
			enabledRoutes: () => [],
			navigate: () => undefined,
			onApplied: () => applied++,
			api: {
				data: never as never,
				invoke: async (_r, _k, _c, body) => {
					calls.push(body);
					return ok(invocation(body.request_id));
				}
			}
		});
		let answer = false;
		const host = dispatcher(item.grants, transport, async () => answer);
		assert.deepEqual(await host.handle(id(1), 'amount.apply', {}), {
			ok: false,
			code: 'preview_required'
		});
		assert.deepEqual(await host.handle(id(2), 'amount.preview', { preview_request_id: id(9) }), {
			ok: false,
			code: 'invalid_params'
		});
		assert.deepEqual(await host.handle(id(3), 'amount.preview', {}), {
			ok: true,
			result: { amount_cents: 2500 }
		});
		assert.deepEqual(await host.handle(id(4), 'amount.apply', {}), { ok: false, code: 'canceled' });
		answer = true;
		assert.deepEqual(await host.handle(id(5), 'amount.apply', {}), {
			ok: true,
			result: { amount_cents: 2500 }
		});
		assert.equal(applied, 1);
		assert.deepEqual(calls[1], {
			package_id: 'pkg-1',
			generation: 7,
			request_id: id(5),
			preview_request_id: id(3),
			capability: 'amount.apply',
			input: {}
		});
		assert.deepEqual(await host.handle(id(6), 'amount.apply', {}), {
			ok: false,
			code: 'preview_required'
		});
	});

	test('rejects mismatched invocation results', async () => {
		const transport = createExtensionTransport({
			projectId: 'p',
			item,
			recordId,
			enabledRoutes: () => [],
			navigate: () => undefined,
			api: {
				data: never as never,
				invoke: async (_r, _k, _c, body) => ok({ ...invocation(body.request_id), generation: 8 })
			}
		});
		assert.deepEqual(await transport('amount.preview', {}, id(1)), { ok: false, code: 'failed' });
	});

	test('protocol 1 keeps the legacy limits and errors', async () => {
		const panel: PluginPanel = {
			package_id: 'pkg-1',
			key: 'order_amount',
			version: '1.0.0',
			name: 'Order amount',
			generation: 7,
			grants: ['amount.preview'],
			html: '',
			web_sha256: '',
			package_sha256: ''
		};
		const responses: ApiResult<PluginInvocation>[] = [];
		const transport = createLegacyTransport({
			recordId,
			panel,
			api: { invoke: async () => responses.shift() as ApiResult<PluginInvocation> }
		});
		const host = createHostDispatcher({
			bridge: 1,
			grants: () => panel.grants,
			context: () => undefined,
			transport
		});
		assert.deepEqual(await host.handle(id(1), 'amount.apply', {}), {
			ok: false,
			code: 'method_unavailable'
		});
		assert.deepEqual(await host.handle(id(2), undefined, {}), {
			ok: false,
			code: 'method_unavailable'
		});
		responses.push(ok(invocation(id(3), 1325)));
		assert.deepEqual(await host.handle(id(3), 'amount.preview', {}), {
			ok: true,
			result: { amount_cents: 1325 }
		});
		assert.deepEqual(await host.handle(id(3), 'amount.preview', {}), { ok: false, code: 'busy' });
		responses.push(err(403));
		assert.deepEqual(await host.handle(id(4), 'amount.preview', {}), {
			ok: false,
			code: 'denied',
			close: 'suspend'
		});
		for (let n = 5; n < 103; n++) {
			responses.push(ok(invocation(id(n))));
			await host.handle(id(n), 'amount.preview', {});
		}
		assert.deepEqual(await host.handle(id(103), 'amount.preview', {}), { ok: false, code: 'busy' });
	});

	test('a disposed dispatcher answers canceled', async () => {
		const host = dispatcher(['ui.display'], never);
		host.dispose();
		assert.deepEqual(await host.handle(id(1), 'host.context', {}), { ok: false, code: 'canceled' });
	});
});
