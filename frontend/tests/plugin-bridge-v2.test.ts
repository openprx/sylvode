import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
	bridgeErrorCode,
	clampFrameHeight,
	parseBridgeV2,
	pluginRequest,
	shellInitMessage,
	type PluginHostContext
} from '../src/lib/plugins/bridge';
import { createHostDispatcher } from '../src/lib/plugins/host-methods';

const requestId = 'dea0894d-cc5d-4c1f-85d1-9694dcef60bb';
const id = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;

const context: PluginHostContext = {
	package: { key: 'order_board', version: '1.0.0' },
	contribution: { id: 'board', kind: 'view', slot: 'project.view', view: 'board' },
	project: { name: 'Demo' },
	form: null,
	record: null,
	route: { state: null },
	locale: 'en',
	theme: 'light'
};

describe('bridge protocol 2 parsing', () => {
	const request = {
		protocol: 2,
		type: 'request',
		request_id: requestId,
		method: 'data.query',
		params: { read: 'orders' }
	};

	test('accepts a well-formed request and the signal messages', () => {
		assert.deepEqual(parseBridgeV2(request), {
			type: 'request',
			requestId,
			method: 'data.query',
			params: { read: 'orders' }
		});
		assert.deepEqual(parseBridgeV2({ protocol: 2, type: 'ready' }), { type: 'ready' });
		assert.deepEqual(parseBridgeV2({ protocol: 2, type: 'close' }), { type: 'close' });
		assert.deepEqual(parseBridgeV2({ protocol: 2, type: 'resize', height: 900 }), {
			type: 'resize',
			height: 900
		});
	});

	test('keeps unknown methods answerable but without a method', () => {
		const parsed = parseBridgeV2({ ...request, method: 'fetch' });
		assert.equal(parsed?.type, 'request');
		assert.equal((parsed as { method?: string }).method, undefined);
		const proto = parseBridgeV2({ ...request, method: '__proto__' });
		assert.equal((proto as { method?: string }).method, undefined);
	});

	test('drops invalid request IDs, other protocols and malformed messages', () => {
		assert.equal(parseBridgeV2({ ...request, request_id: 'foreign' }), undefined);
		assert.equal(
			parseBridgeV2({ ...request, request_id: 'dea0894d-cc5d-1c1f-85d1-9694dcef60bb' }),
			undefined
		);
		assert.equal(parseBridgeV2({ ...request, request_id: 7 }), undefined);
		assert.equal(parseBridgeV2({ ...request, protocol: 1 }), undefined);
		assert.equal(parseBridgeV2({ protocol: 2, type: 'resize', height: 1.5 }), undefined);
		assert.equal(parseBridgeV2({ protocol: 2, type: 'resize', height: '400' }), undefined);
		assert.equal(parseBridgeV2({ protocol: 2, type: 'unknown' }), undefined);
		assert.equal(parseBridgeV2(null), undefined);
		assert.equal(parseBridgeV2([request]), undefined);
	});

	test('reports non-object params as missing', () => {
		const parsed = parseBridgeV2({ ...request, params: ['orders'] });
		assert.equal(parsed?.type, 'request');
		assert.equal((parsed as { params?: unknown }).params, undefined);
	});

	test('leaves protocol 1 parsing untouched', () => {
		const legacy = {
			protocol: 1,
			type: 'request',
			request_id: requestId,
			method: 'amount.preview',
			input: {}
		};
		assert.deepEqual(pluginRequest(legacy), { method: 'amount.preview', requestId });
		assert.equal(pluginRequest({ ...legacy, protocol: 2 }), undefined);
	});

	test('clamps heights and normalises error codes', () => {
		assert.equal(clampFrameHeight(10, 320, 4000), 320);
		assert.equal(clampFrameHeight(99999, 160, 800), 800);
		assert.equal(clampFrameHeight(500, 160, 800), 500);
		assert.equal(bridgeErrorCode('stale'), 'stale');
		assert.equal(bridgeErrorCode('anything'), 'failed');
	});

	test('builds protocol 2 shell init with context and without placement', () => {
		assert.deepEqual(shellInitMessage({ bridge: 2, html: 'x', grants: ['ui.display'], context }), {
			protocol: 1,
			type: 'sylvode-plugin-shell-init',
			html: 'x',
			grants: ['ui.display'],
			bridge: 2,
			context
		});
		assert.deepEqual(
			shellInitMessage({ bridge: 1, html: 'x', grants: [], placement: { id: 'a', slot: 'b' } }),
			{
				protocol: 1,
				type: 'sylvode-plugin-shell-init',
				html: 'x',
				grants: [],
				placement: { id: 'a', slot: 'b' }
			}
		);
		assert.equal('bridge' in shellInitMessage({ bridge: 1, html: 'x', grants: [] }), false);
	});
});

describe('bridge protocol 2 request ledger', () => {
	test('rejects a reused request ID', async () => {
		const dispatcher = createHostDispatcher({
			bridge: 2,
			grants: () => ['ui.display'],
			context: () => context,
			transport: async () => ({ ok: true, result: null })
		});
		assert.deepEqual(await dispatcher.handle(requestId, 'host.context', {}), {
			ok: true,
			result: context
		});
		assert.deepEqual(await dispatcher.handle(requestId, 'host.context', {}), {
			ok: false,
			code: 'invalid_params'
		});
	});

	test('caps requests per frame at 500, including rejected ones', async () => {
		const dispatcher = createHostDispatcher({
			bridge: 2,
			grants: () => ['ui.display'],
			context: () => context,
			transport: async () => ({ ok: true, result: null })
		});
		for (let n = 0; n < 500; n++) {
			const outcome = await dispatcher.handle(id(n), n % 2 ? 'host.context' : undefined, {});
			assert.notEqual(!outcome.ok && outcome.code, 'busy');
		}
		assert.deepEqual(await dispatcher.handle(id(500), 'host.context', {}), {
			ok: false,
			code: 'busy'
		});
	});

	test('allows at most four data requests in flight', async () => {
		const pending: (() => void)[] = [];
		const dispatcher = createHostDispatcher({
			bridge: 2,
			grants: () => ['ui.display', 'read:orders'],
			context: () => context,
			transport: () =>
				new Promise((resolve) => pending.push(() => resolve({ ok: true, result: 'rows' })))
		});
		const running = [0, 1, 2, 3].map((n) =>
			dispatcher.handle(id(n), 'data.describe', { read: 'orders' })
		);
		assert.deepEqual(await dispatcher.handle(id(4), 'data.query', { read: 'orders' }), {
			ok: false,
			code: 'busy'
		});
		await Promise.resolve();
		for (const release of pending) release();
		for (const outcome of await Promise.all(running))
			assert.deepEqual(outcome, { ok: true, result: 'rows' });
		const after = dispatcher.handle(id(5), 'data.query', { read: 'orders' });
		await Promise.resolve();
		await Promise.resolve();
		pending.at(-1)?.();
		assert.deepEqual(await after, { ok: true, result: 'rows' });
	});
});
