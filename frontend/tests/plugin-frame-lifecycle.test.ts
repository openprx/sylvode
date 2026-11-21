import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { ApiResult } from '../src/lib/api/client';
import {
	createFrameLifecycle,
	suspendGraceMs,
	type FramePort,
	type FrameTimers
} from '../src/lib/plugins/frame-lifecycle';
import {
	createExtensionTransport,
	createHostDispatcher,
	type HostTransport
} from '../src/lib/plugins/host-methods';

const id = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;
const event = { protocol: 2, type: 'event', name: 'suspended', data: null };

function harness(options: { transport?: HostTransport; grace?: number } = {}) {
	const log: string[] = [];
	const transported: string[] = [];
	const timers = new Map<number, { cb: () => void; ms: number }>();
	let next = 0;
	const fakeTimers: FrameTimers = {
		set: (cb, ms) => {
			timers.set(++next, { cb, ms });
			return next;
		},
		clear: (handle) => {
			timers.delete(handle as number);
		}
	};
	const port: FramePort = {
		onmessage: () => undefined,
		postMessage: (message) => log.push(`post:${(message as { name?: string }).name}`),
		close: () => log.push('close')
	};
	const dispatcher = createHostDispatcher({
		grants: () => ['read:orders', 'ui.display'],
		context: () => undefined,
		transport:
			options.transport ??
			(async (method) => {
				transported.push(method);
				return { ok: true, result: {} };
			})
	});
	const lifecycle = createFrameLifecycle({
		port,
		dispose: () => {
			log.push('dispose');
			dispatcher.dispose();
		},
		removeFrame: () => log.push('remove'),
		timers: fakeTimers,
		...(options.grace ? { graceMs: options.grace } : {})
	});
	return { log, transported, timers, port, dispatcher, lifecycle };
}

describe('frame lifecycle', () => {
	test('posts suspended before close; the frame stays until the grace period ends', () => {
		const h = harness();
		h.lifecycle.suspend(event);
		assert.deepEqual(h.log, ['dispose', 'post:suspended']);
		assert.equal(h.lifecycle.state, 'suspending');
		assert.equal(h.port.onmessage, null);
		const [timer] = [...h.timers.values()];
		assert.equal(timer.ms, suspendGraceMs);
		timer.cb();
		assert.deepEqual(h.log, ['dispose', 'post:suspended', 'close', 'remove']);
		assert.equal(h.lifecycle.state, 'closed');
	});

	test('removes the frame after the grace period without any acknowledgement', () => {
		const h = harness({ grace: 50 });
		h.lifecycle.suspend(event);
		assert.equal([...h.timers.values()][0].ms, 50);
		assert.equal(h.log.includes('remove'), false);
		[...h.timers.values()][0].cb();
		assert.equal(h.log.at(-1), 'remove');
		assert.equal(h.timers.size, 0);
	});

	test('requests after suspension starts are refused and never reach the transport', async () => {
		const h = harness();
		h.lifecycle.suspend(event);
		for (let n = 1; n <= 3; n++)
			assert.deepEqual(await h.dispatcher.handle(id(n), 'data.query', { read: 'orders' }), {
				ok: false,
				code: 'canceled'
			});
		assert.deepEqual(await h.dispatcher.handle(id(9), 'host.navigate', { target: 'project' }), {
			ok: false,
			code: 'canceled'
		});
		assert.deepEqual(h.transported, []);
	});

	test('a request queued before suspension does not reach the transport after it', async () => {
		const h = harness();
		const pending = h.dispatcher.handle(id(1), 'data.query', { read: 'orders' });
		h.lifecycle.suspend(event);
		assert.deepEqual(await pending, { ok: false, code: 'canceled' });
		assert.deepEqual(h.transported, []);
	});

	test('an in-flight request gets no result after suspension', async () => {
		let release: (value: { ok: true; result: unknown }) => void = () => undefined;
		const h = harness({ transport: () => new Promise((resolve) => (release = resolve)) });
		const pending = h.dispatcher.handle(id(1), 'data.query', { read: 'orders' });
		await Promise.resolve();
		await Promise.resolve();
		h.lifecycle.suspend(event);
		release({ ok: true, result: { rows: [] } });
		assert.deepEqual(await pending, { ok: false, code: 'canceled' });
	});

	test('destroy during the grace period closes at once and the timer is cleared', () => {
		const h = harness();
		h.lifecycle.suspend(event);
		h.lifecycle.close();
		assert.deepEqual(h.log, ['dispose', 'post:suspended', 'close', 'remove']);
		assert.equal(h.timers.size, 0);
	});

	test('destroy of a live frame disposes, closes and removes without a suspended event', () => {
		const h = harness();
		h.lifecycle.close();
		assert.deepEqual(h.log, ['dispose', 'close', 'remove']);
		assert.equal(h.port.onmessage, null);
	});

	test('repeated suspend and close never double-close', () => {
		const h = harness();
		h.lifecycle.suspend(event);
		h.lifecycle.suspend(event);
		h.lifecycle.close();
		h.lifecycle.close();
		h.lifecycle.suspend(event);
		assert.deepEqual(h.log, ['dispose', 'post:suspended', 'close', 'remove']);
		assert.equal(h.timers.size, 0);
	});

	test('a port that throws on post or close still ends removed', () => {
		const h = harness();
		h.port.postMessage = () => {
			throw new Error('gone');
		};
		h.port.close = () => {
			throw new Error('gone');
		};
		h.lifecycle.suspend(event);
		[...h.timers.values()][0].cb();
		assert.equal(h.lifecycle.state, 'closed');
		assert.equal(h.log.at(-1), 'remove');
	});
});

describe('host.navigate reply ordering', () => {
	const ok = <T>(data: T): ApiResult<T> => ({ code: 0, message: 'ok', data });
	const item = {
		package_id: 'pkg',
		key: 'k',
		version: '1.0.0',
		generation: 1,
		contribution: { id: 'c' }
	} as never;
	const macrotask = () => new Promise((resolve) => setTimeout(resolve, 5));

	function setup(navigate: () => void | Promise<void>) {
		const order: string[] = [];
		const data = async () => {
			order.push('data');
			return ok({});
		};
		const transport = createExtensionTransport({
			projectId: 'p',
			item,
			enabledRoutes: () => ['board'],
			navigate: async (target) => {
				order.push(`navigate:${target.target}`);
				await navigate();
			},
			api: { data: data as never, invoke: data as never }
		});
		const dispatcher = createHostDispatcher({
			grants: () => ['ui.display', 'read:orders'],
			context: () => undefined,
			transport
		});
		return { order, dispatcher };
	}

	test('the reply is delivered before navigation starts', async () => {
		const { order, dispatcher } = setup(() => undefined);
		const reply = await dispatcher.handle(id(1), 'host.navigate', { target: 'project' });
		order.push('reply');
		assert.deepEqual(reply, { ok: true, result: { navigated: true } });
		await macrotask();
		assert.deepEqual(order, ['reply', 'navigate:project']);
	});

	test('other requests are refused while an accepted navigation is pending, then resume', async () => {
		let finish: () => void = () => undefined;
		const { order, dispatcher } = setup(() => new Promise<void>((resolve) => (finish = resolve)));
		await dispatcher.handle(id(1), 'host.navigate', { target: 'project' });
		await macrotask();
		assert.deepEqual(await dispatcher.handle(id(2), 'data.query', { read: 'orders' }), {
			ok: false,
			code: 'canceled'
		});
		assert.deepEqual(order, ['navigate:project']);
		finish();
		await macrotask();
		assert.deepEqual(await dispatcher.handle(id(3), 'data.query', { read: 'orders' }), {
			ok: true,
			result: {}
		});
	});

	test('a second navigate during the pending window is refused and does not navigate', async () => {
		let finish: () => void = () => undefined;
		const { order, dispatcher } = setup(() => new Promise<void>((resolve) => (finish = resolve)));
		const first = dispatcher.handle(id(1), 'host.navigate', { target: 'view', route: 'board' });
		// Concurrent with the first call: the dispatcher's own limit.
		const concurrent = dispatcher.handle(id(2), 'host.navigate', { target: 'project' });
		assert.deepEqual(await first, { ok: true, result: { navigated: true } });
		assert.deepEqual(await concurrent, { ok: false, code: 'busy' });
		// After the reply, before the navigation settled: the transport refuses it.
		assert.deepEqual(await dispatcher.handle(id(3), 'host.navigate', { target: 'project' }), {
			ok: false,
			code: 'canceled'
		});
		await macrotask();
		assert.deepEqual(order, ['navigate:view']);
		finish();
		await macrotask();
		// A same-route navigation keeps the frame, so it must be served again once settled.
		assert.equal((await dispatcher.handle(id(4), 'data.query', { read: 'orders' })).ok, true);
	});

	test('a navigation that never settles only wedges its own transport', async () => {
		const stuck = setup(() => new Promise<void>(() => undefined));
		await stuck.dispatcher.handle(id(1), 'host.navigate', { target: 'project' });
		await macrotask();
		assert.equal(
			(await stuck.dispatcher.handle(id(2), 'data.query', { read: 'orders' })).ok,
			false
		);
		// A reopened view creates a new transport; nothing is shared between the two.
		const fresh = setup(() => undefined);
		assert.equal((await fresh.dispatcher.handle(id(1), 'data.query', { read: 'orders' })).ok, true);
	});

	test('a navigation that fails to start does not reject or wedge the transport', async () => {
		const { dispatcher } = setup(() => Promise.reject(new Error('offline')));
		await dispatcher.handle(id(1), 'host.navigate', { target: 'project' });
		await macrotask();
		assert.equal((await dispatcher.handle(id(2), 'data.query', { read: 'orders' })).ok, true);
	});
});
