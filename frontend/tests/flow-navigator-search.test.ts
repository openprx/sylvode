/**
 * Navigator Flow search, projection-lag badge and standalone Collection create
 * (`ui-surface-v1.md` "后续版本 UI 派生" v0.5/v0.6; `rest-api-v1.md` v0.5 `GET .../flow/search`,
 * `GET .../flow/projection-lag`, v0.6 `POST .../flow/objects` `object_type=collection`).
 *
 * - search: the real `flowApi.searchObjects` request against a mocked `fetch`; debounce (a burst
 *   inside the window sends one request); a new request aborts the previous one and a late old
 *   response never overwrites the new result; stale badge; keyboard index bounds;
 *   `require_current` failing closed is not downgraded to `allow_stale`; errors by `error_code`.
 * - lag badge: `max_lag == 0` is "synced", a rejected read is "unavailable", 30s refresh.
 * - Collection create: the exact request body, one key and one id set per intent (a retry
 *   resends the identical body), typed errors, `server_rejected` not retryable.
 *
 * Run standalone: `bun tests/flow-navigator-search.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	FlowNavigatorSearch,
	SEARCH_DEBOUNCE_MS,
	SEARCH_GENERIC_ERROR_KEY,
	SEARCH_STALE_FRONTIER_KEY,
	SEARCH_TOO_BROAD_KEY,
	SEARCH_TOO_LONG_KEY,
	isStaleHit,
	nextActiveIndex,
	parseSnippet,
	searchFailureKey,
	searchRowFromHit,
	type SearchReads
} from '../src/lib/flow/navigator-search';
import { FlowLagBadge, LAG_REFRESH_MS, lagBadgeFromResult } from '../src/lib/flow/navigator-lag';
import {
	CreateCollectionSession,
	validateCollectionDraft,
	type CollectionCreator,
	type CollectionDraft
} from '../src/lib/flow/create-collection';
import { FlowCommandService } from '../src/lib/flow/command-service';
import {
	flowApi,
	type AcceptedChange,
	type FlowSearchHit,
	type FlowSearchQuery,
	type FlowSearchResponse
} from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const WS = 'ws-1';
const ROOT = new URL('..', import.meta.url).pathname;

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function hit(id: string, extra: Partial<FlowSearchHit> = {}): FlowSearchHit {
	return {
		object: { id, object_type: 'page', title: `Title ${id}`, lifecycle_status: 'active' },
		matched_fields: ['title'],
		snippets: { title: `<mark>Title</mark> ${id}` },
		indexed_seq: 3,
		head_seq: 3,
		projection_lag: 0,
		stale: false,
		...extra
	};
}

function response(ids: string[], frontier: Partial<FlowSearchResponse['index_frontier']> = {}) {
	return ok<FlowSearchResponse>({
		items: ids.map((id) => hit(id)),
		index_frontier: { indexed_seq: 3, head_seq: 3, lag: 0, stale: false, ...frontier }
	});
}

interface Pending {
	query: FlowSearchQuery;
	signal: AbortSignal | undefined;
	resolve: (value: ApiResult<FlowSearchResponse>) => void;
}

/** A search read whose responses are released by the test, plus a manual timer. */
function harness() {
	const calls: Pending[] = [];
	const timers: Array<{ callback: () => void; ms: number; cleared: boolean }> = [];
	const reads: SearchReads = {
		search: (workspaceId, query, signal) => {
			assertEqual(workspaceId, WS, 'workspace id');
			return new Promise((resolve) => calls.push({ query: { ...query }, signal, resolve }));
		}
	};
	const search = new FlowNavigatorSearch(WS, () => {}, {
		reads,
		setTimer: (callback, ms) => {
			const timer = { callback, ms, cleared: false };
			timers.push(timer);
			return timer;
		},
		clearTimer: (handle) => {
			(handle as { cleared: boolean }).cleared = true;
		}
	});
	const flushTimers = () => {
		for (const timer of timers.splice(0)) if (!timer.cleared) timer.callback();
	};
	return { search, calls, timers, flushTimers };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

const suite = new Suite('flow-navigator-search');

// ---- search: request shape ------------------------------------------------------------------

await suite.checkAsync(
	'flowApi.searchObjects sends the contract query and the abort signal',
	async () => {
		const seen: Array<{ url: string; signal: AbortSignal | null | undefined; method?: string }> =
			[];
		const original = globalThis.fetch;
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			const raw = String(input);
			seen.push({
				url: raw.startsWith('http') ? new URL(raw).pathname + new URL(raw).search : raw,
				signal: init?.signal,
				method: init?.method
			});
			return new Response(JSON.stringify(response([])), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		const controller = new AbortController();
		try {
			await flowApi.searchObjects(
				WS,
				{ q: 'needle words', all_visible: true, freshness: 'require_current', limit: 20 },
				controller.signal
			);
			await flowApi.searchObjects(WS, { q: 'x', project_id: 'p-1', cursor: 'c1' });
		} finally {
			globalThis.fetch = original;
		}
		assertDeepEqual(
			seen.map((s) => s.url),
			[
				`/api/v1/workspaces/${WS}/flow/search?q=needle+words&all_visible=true&freshness=require_current&limit=20`,
				`/api/v1/workspaces/${WS}/flow/search?q=x&project_id=p-1&cursor=c1`
			],
			'query string'
		);
		assertEqual(seen[0].method, 'GET', 'GET');
		assert(seen[0].signal === controller.signal, 'the caller signal reaches fetch');
	}
);

await suite.checkAsync(
	'the controller asks for a human all_visible, allow_stale search by default',
	async () => {
		const h = harness();
		h.search.setQuery('  needle ');
		h.flushTimers();
		assertEqual(h.calls.length, 1, 'one request');
		assertDeepEqual(
			h.calls[0].query,
			{ q: 'needle', all_visible: true, freshness: 'allow_stale', limit: 20 },
			'request query (trimmed q, one scope)'
		);
	}
);

// ---- search: debounce -----------------------------------------------------------------------

await suite.checkAsync(
	'keystrokes inside the debounce window send exactly one request',
	async () => {
		const h = harness();
		h.search.setQuery('n');
		h.search.setQuery('ne');
		h.search.setQuery('nee');
		assertEqual(h.calls.length, 0, 'nothing leaves while typing');
		assertEqual(h.search.snapshot.status, 'pending', 'pending while debouncing');
		assert(
			h.timers.every((timer) => timer.ms === SEARCH_DEBOUNCE_MS),
			'every timer uses the 200ms window'
		);
		assertEqual(SEARCH_DEBOUNCE_MS, 200, 'contract: 200ms');
		h.flushTimers();
		assertEqual(h.calls.length, 1, 'one request after the window');
		assertEqual(h.calls[0].query.q, 'nee', 'with the last query');
	}
);

await suite.checkAsync('with real timers a burst sends one request after ~200ms', async () => {
	const calls: string[] = [];
	const search = new FlowNavigatorSearch(WS, () => {}, {
		reads: {
			search: async (_ws, query) => {
				calls.push(query.q);
				return response(['a']);
			}
		}
	});
	search.setQuery('a');
	await new Promise((resolve) => setTimeout(resolve, 50));
	search.setQuery('ab');
	await new Promise((resolve) => setTimeout(resolve, 50));
	search.setQuery('abc');
	await new Promise((resolve) => setTimeout(resolve, 120));
	assertDeepEqual(calls, [], 'nothing sent 120ms after the last keystroke');
	await new Promise((resolve) => setTimeout(resolve, 150));
	assertDeepEqual(calls, ['abc'], 'one request with the final query');
	search.dispose();
});

// ---- search: abort --------------------------------------------------------------------------

await suite.checkAsync(
	'a new request aborts the old one; the late old response is dropped',
	async () => {
		const h = harness();
		h.search.setQuery('old');
		h.flushTimers();
		h.search.setQuery('new');
		h.flushTimers();
		assertEqual(h.calls.length, 2, 'two requests');
		assert(h.calls[0].signal?.aborted === true, 'the first request was aborted');
		assert(h.calls[1].signal?.aborted === false, 'the current request is live');
		h.calls[1].resolve(response(['new-1', 'new-2']));
		await tick();
		h.calls[0].resolve(response(['old-1']));
		await tick();
		assertDeepEqual(
			h.search.snapshot.rows.map((row) => row.id),
			['new-1', 'new-2'],
			'the new result stands'
		);
		assertEqual(h.search.snapshot.status, 'ready', 'ready');
	}
);

await suite.checkAsync('an old response arriving before the new one never shows', async () => {
	const h = harness();
	h.search.setQuery('old');
	h.flushTimers();
	h.search.setQuery('new');
	h.flushTimers();
	h.calls[0].resolve(response(['old-1']));
	await tick();
	assertDeepEqual(h.search.snapshot.rows, [], 'the old answer is not rendered');
	assertEqual(h.search.snapshot.status, 'loading', 'still loading the new query');
	h.calls[1].resolve(response(['new-1']));
	await tick();
	assertDeepEqual(
		h.search.snapshot.rows.map((row) => row.id),
		['new-1'],
		'then the new one'
	);
});

await suite.checkAsync('clearing the query aborts the in-flight request and resets', async () => {
	const h = harness();
	h.search.setQuery('needle');
	h.flushTimers();
	h.search.setQuery('   ');
	assert(h.calls[0].signal?.aborted === true, 'aborted');
	h.calls[0].resolve(response(['late']));
	await tick();
	assertEqual(h.search.snapshot.status, 'idle', 'idle');
	assertDeepEqual(h.search.snapshot.rows, [], 'no rows');
});

await suite.checkAsync(
	'a query over 256 characters is refused locally with no request',
	async () => {
		const h = harness();
		h.search.setQuery('字'.repeat(257));
		h.flushTimers();
		assertEqual(h.calls.length, 0, 'no request');
		assertEqual(h.search.snapshot.failureKey, SEARCH_TOO_LONG_KEY, 'too long key');
		h.search.setQuery('字'.repeat(256));
		h.flushTimers();
		assertEqual(h.calls.length, 1, '256 characters are allowed');
	}
);

// ---- search: stale badge --------------------------------------------------------------------

suite.check('stale badge: the server flag or a positive projection lag', () => {
	assertEqual(isStaleHit({ stale: false, projection_lag: 0 }), false, 'current');
	assertEqual(isStaleHit({ stale: true, projection_lag: 0 }), true, 'flagged');
	assertEqual(isStaleHit({ stale: false, projection_lag: 2 }), true, 'lagging');
	assertEqual(isStaleHit({ stale: false, projection_lag: Number.NaN }), false, 'NaN is not lag');
	const row = searchRowFromHit(hit('a', { stale: true, projection_lag: 4 }));
	assertEqual(row.stale, true, 'row stale');
	assertEqual(row.lag, 4, 'row lag');
	assertEqual(searchRowFromHit(hit('b')).stale, false, 'current row');
});

await suite.checkAsync(
	'the response frontier is surfaced and rows keep their own stale flag',
	async () => {
		const h = harness();
		h.search.setQuery('needle');
		h.flushTimers();
		h.calls[0].resolve(
			ok<FlowSearchResponse>({
				items: [hit('a', { projection_lag: 1 }), hit('b')],
				index_frontier: { indexed_seq: 2, head_seq: 5, lag: 3, stale: true }
			})
		);
		await tick();
		const snap = h.search.snapshot;
		assertEqual(snap.frontierStale, true, 'frontier stale');
		assertEqual(snap.frontierLag, 3, 'frontier lag');
		assertDeepEqual(
			snap.rows.map((row) => row.stale),
			[true, false],
			'per-row stale'
		);
	}
);

suite.check('snippets: only <mark> is markup; escaped text is unescaped as plain text', () => {
	assertDeepEqual(
		parseSnippet('a &lt;mark&gt; b <mark>hit</mark> &amp; <mark>two</mark>'),
		[
			{ text: 'a <mark> b ', match: false },
			{ text: 'hit', match: true },
			{ text: ' & ', match: false },
			{ text: 'two', match: true }
		],
		'segments'
	);
	assertDeepEqual(parseSnippet(''), [], 'empty');
	const row = searchRowFromHit(
		hit('c', { snippets: { title: '<mark>T</mark>', plain_text: 'body <mark>x</mark>' } })
	);
	assertDeepEqual(
		row.snippet.map((s) => s.text),
		['body ', 'x'],
		'body snippet preferred'
	);
});

// ---- search: keyboard -----------------------------------------------------------------------

suite.check('keyboard index bounds: clamp at both ends, Home/End, no rows', () => {
	assertEqual(nextActiveIndex(-1, 3, 'ArrowDown'), 0, 'down from none');
	assertEqual(nextActiveIndex(0, 3, 'ArrowDown'), 1, 'down');
	assertEqual(nextActiveIndex(2, 3, 'ArrowDown'), 2, 'down at last stays');
	assertEqual(nextActiveIndex(0, 3, 'ArrowUp'), 0, 'up at first stays');
	assertEqual(nextActiveIndex(2, 3, 'ArrowUp'), 1, 'up');
	assertEqual(nextActiveIndex(-1, 3, 'ArrowUp'), 0, 'up from none');
	assertEqual(nextActiveIndex(1, 3, 'Home'), 0, 'home');
	assertEqual(nextActiveIndex(0, 3, 'End'), 2, 'end');
	assertEqual(nextActiveIndex(5, 3, 'ArrowUp'), 2, 'out-of-range index is clamped');
	assertEqual(nextActiveIndex(-1, 0, 'ArrowDown'), -1, 'no rows: none');
	assertEqual(nextActiveIndex(-1, 0, 'End'), -1, 'no rows: none');
	assertEqual(nextActiveIndex(0, 3, 'a'), null, 'not a navigation key');
	assertEqual(nextActiveIndex(0, 3, 'Enter'), null, 'Enter is not navigation');
});

await suite.checkAsync('results start on the first row and the keys move it', async () => {
	const h = harness();
	h.search.setQuery('needle');
	h.flushTimers();
	h.calls[0].resolve(response(['a', 'b', 'c']));
	await tick();
	assertEqual(h.search.snapshot.activeIndex, 0, 'first active');
	assert(h.search.handleKey('ArrowDown'), 'consumed');
	assert(h.search.handleKey('ArrowDown'), 'consumed');
	assert(h.search.handleKey('ArrowDown'), 'consumed at end');
	assertEqual(h.search.snapshot.activeIndex, 2, 'clamped at last');
	assertEqual(h.search.activeRow?.id, 'c', 'active row');
	assertEqual(h.search.handleKey('x'), false, 'other keys are not consumed');
	h.search.setQuery('none');
	h.flushTimers();
	h.calls[1].resolve(response([]));
	await tick();
	assertEqual(h.search.snapshot.activeIndex, -1, 'empty result: nothing active');
	assertEqual(h.search.activeRow, null, 'no active row');
});

// ---- search: require_current and errors -----------------------------------------------------

await suite.checkAsync(
	'require_current failing closed is shown and NOT retried as allow_stale',
	async () => {
		const h = harness();
		h.search.setQuery('needle');
		h.flushTimers();
		h.calls[0].resolve(response(['a']));
		await tick();
		h.search.setFreshness('require_current');
		assertEqual(h.calls.length, 2, 'the toggle re-runs at once');
		assertEqual(h.calls[1].query.freshness, 'require_current', 'require_current sent');
		h.calls[1].resolve({
			code: 409,
			// The message is deliberately misleading: classification must not read it.
			message: 'ok, showing allow_stale results',
			data: null,
			error_code: 'stale_frontier',
			details: { current_seq: 9 }
		});
		await tick();
		await tick();
		const snap = h.search.snapshot;
		assertEqual(snap.status, 'error', 'error');
		assertEqual(snap.failureKey, SEARCH_STALE_FRONTIER_KEY, 'stale frontier copy');
		assertEqual(snap.failureCode, 'stale_frontier', 'code');
		assertDeepEqual(snap.rows, [], 'the earlier allow_stale rows are not kept');
		assertEqual(h.calls.length, 2, 'no silent allow_stale request');
		assertEqual(snap.freshness, 'require_current', 'the toggle stays on');
	}
);

suite.check('errors are classified by error_code only', () => {
	assertEqual(
		searchFailureKey({ code: 400, error_code: 'limit_exceeded', details: {} }).key,
		SEARCH_TOO_BROAD_KEY,
		'scan budget'
	);
	assertEqual(
		searchFailureKey({ code: 403, error_code: 'forbidden' }).key,
		'flow.error.forbidden',
		'forbidden'
	);
	assertEqual(
		searchFailureKey({ code: 503, error_code: 'server_draining', details: { reason: 'drain' } })
			.key,
		'flow.error.server_draining.drain',
		'drain reason'
	);
	const legacy = searchFailureKey({ code: 409, error_code: undefined });
	assertEqual(legacy.key, SEARCH_GENERIC_ERROR_KEY, 'legacy envelope: generic');
	assertEqual(legacy.code, null, 'no code');
});

// ---- lag badge ------------------------------------------------------------------------------

suite.check('lag badge state from the projection-lag read', () => {
	assertDeepEqual(
		lagBadgeFromResult(ok({ max_lag: 0, p95_lag: 0, items: [] })),
		{ kind: 'synced' },
		'zero is synced'
	);
	assertDeepEqual(
		lagBadgeFromResult(ok({ max_lag: 7, p95_lag: 3, items: [] })),
		{ kind: 'lagging', maxLag: 7, p95Lag: 3 },
		'lagging'
	);
	assertDeepEqual(
		lagBadgeFromResult({ code: 403, message: 'synced', data: null, error_code: 'forbidden' }),
		{ kind: 'unavailable', errorCode: 'forbidden' },
		'rejected read is unavailable, never synced'
	);
	assertDeepEqual(
		lagBadgeFromResult(ok({ max_lag: -1, p95_lag: 0, items: [] })),
		{ kind: 'unavailable', errorCode: null },
		'malformed numbers'
	);
});

await suite.checkAsync('lag badge reads limit=1 now and every 30s, and stops', async () => {
	const queries: unknown[] = [];
	const intervals: Array<{ callback: () => void; ms: number; cleared: boolean }> = [];
	const states: string[] = [];
	let lag = 2;
	const badge = new FlowLagBadge(WS, (state) => states.push(state.kind), {
		reads: {
			getProjectionLag: async (workspaceId, query) => {
				assertEqual(workspaceId, WS, 'workspace');
				queries.push(query);
				return ok({ max_lag: lag, p95_lag: lag, items: [] });
			}
		},
		setInterval: (callback, ms) => {
			const handle = { callback, ms, cleared: false };
			intervals.push(handle);
			return handle;
		},
		clearInterval: (handle) => {
			(handle as { cleared: boolean }).cleared = true;
		}
	});
	badge.start();
	await tick();
	assertEqual(LAG_REFRESH_MS, 30_000, '30s');
	assertEqual(intervals.length, 1, 'one interval');
	assertEqual(intervals[0].ms, 30_000, 'every 30s');
	assertDeepEqual(queries, [{ limit: 1 }], 'aggregate read');
	lag = 0;
	intervals[0].callback();
	await tick();
	assertDeepEqual(states, ['lagging', 'synced'], 'refreshed');
	badge.stop();
	assert(intervals[0].cleared, 'stop clears the interval');
});

// ---- Collection create ----------------------------------------------------------------------

function draft(extra: Partial<CollectionDraft> = {}): CollectionDraft {
	return {
		title: ' Roadmap ',
		fieldLabels: ['Name'],
		viewType: 'table',
		viewName: 'Table',
		...extra
	};
}

function accepted(id: string): AcceptedChange {
	return { object: { id, object_type: 'collection' } } as unknown as AcceptedChange;
}

function creator(replies: Array<AcceptedChange | Error | object>) {
	const calls: Array<{ input: unknown; key: string | undefined }> = [];
	const commands: CollectionCreator = {
		createObject: async (workspaceId, input, options) => {
			assertEqual(workspaceId, WS, 'workspace');
			calls.push({ input: JSON.parse(JSON.stringify(input)), key: options?.idempotencyKey });
			const reply = replies.shift();
			if (reply && 'object' in reply) return reply as AcceptedChange;
			throw reply;
		}
	};
	let n = 0;
	const session = new CreateCollectionSession(WS, commands, () => `id-${++n}`);
	return { session, calls };
}

await suite.checkAsync(
	'collection create body: type, trimmed title, text fields, initial view',
	async () => {
		const h = creator([accepted('col-1')]);
		const outcome = await h.session.submit(
			draft({ fieldLabels: ['Name', ' Owner '], viewType: 'board', viewName: 'Board' })
		);
		assertEqual(outcome.kind, 'created', 'created');
		assertDeepEqual(
			h.calls[0].input,
			{
				object_type: 'collection',
				title: 'Roadmap',
				initial_fields: [
					{ field_id: 'id-2', label: 'Name', field_type: 'text', config: {}, index: 0 },
					{ field_id: 'id-3', label: 'Owner', field_type: 'text', config: {}, index: 1 }
				],
				initial_view: { view_id: 'id-4', name: 'Board', view_type: 'board', config: {}, index: 0 }
			},
			'request body'
		);
		assertEqual(h.calls[0].key, 'id-1', 'idempotency key');
	}
);

await suite.checkAsync(
	'no fields is allowed: initial_fields is empty, the view is still sent',
	async () => {
		const h = creator([accepted('col-1')]);
		await h.session.submit(draft({ fieldLabels: [] }));
		const body = h.calls[0].input as {
			initial_fields: unknown[];
			initial_view: { view_type: string };
		};
		assertDeepEqual(body.initial_fields, [], 'empty fields');
		assertEqual(body.initial_view.view_type, 'table', 'table view');
	}
);

await suite.checkAsync(
	'a retry of the same intent resends the same key and the same ids',
	async () => {
		const transient = {
			code: 'server_draining',
			recoverable: true,
			details: { reason: 'contention' }
		};
		const h = creator([transient, accepted('col-1')]);
		const first = await h.session.submit(draft());
		assertEqual(first.kind, 'failed', 'first failed');
		if (first.kind === 'failed') {
			assertEqual(first.key, 'flow.error.server_draining.contention', 'typed key');
			assertEqual(first.retryable, true, 'retryable');
		}
		const second = await h.session.submit(draft());
		assertEqual(second.kind, 'created', 'retry created');
		assertEqual(h.calls[1].key, h.calls[0].key, 'same key');
		assertDeepEqual(h.calls[1].input, h.calls[0].input, 'identical body (same field/view ids)');
	}
);

await suite.checkAsync(
	'after a successful create the same form is a new intent: new key and new ids',
	async () => {
		const h = creator([accepted('col-1'), accepted('col-2')]);
		assertEqual((await h.session.submit(draft())).kind, 'created', 'first created');
		assertEqual((await h.session.submit(draft())).kind, 'created', 'second created');
		assert(h.calls[1].key !== h.calls[0].key, 'a second Collection is not a replay of the first');
		assert(
			JSON.stringify(h.calls[1].input) !== JSON.stringify(h.calls[0].input),
			'and gets its own field/view ids'
		);
	}
);

await suite.checkAsync(
	'editing the form after a failure is a new intent: new key and new ids',
	async () => {
		const h = creator([{ code: 'invalid_update', recoverable: false }, accepted('col-1')]);
		await h.session.submit(draft());
		await h.session.submit(draft({ title: 'Roadmap 2' }));
		assert(h.calls[1].key !== h.calls[0].key, 'new key');
		const ids = (call: { input: unknown }) =>
			(call.input as { initial_fields: Array<{ field_id: string }> }).initial_fields[0].field_id;
		assert(ids(h.calls[1]) !== ids(h.calls[0]), 'new field id');
	}
);

await suite.checkAsync('server_rejected is permanent and validation sends nothing', async () => {
	const h = creator([{ code: 'server_rejected', recoverable: false }]);
	const outcome = await h.session.submit(draft());
	assert(outcome.kind === 'failed' && !outcome.retryable, 'not retryable');
	if (outcome.kind === 'failed') assertEqual(outcome.key, 'flow.error.server_rejected', 'key');
	const before = h.calls.length;
	assertEqual((await h.session.submit(draft({ title: '  ' }))).kind, 'invalid', 'empty title');
	assertEqual(
		(await h.session.submit(draft({ fieldLabels: ['Name', ' '] }))).kind,
		'invalid',
		'empty field label'
	);
	assertEqual(h.calls.length, before, 'no request for an invalid draft');
	assertEqual(
		validateCollectionDraft(draft({ title: 'x'.repeat(501) })),
		'title_too_long',
		'length'
	);
	assertEqual(validateCollectionDraft(draft({ title: 'x'.repeat(500) })), 'ok', '500 is fine');
});

await suite.checkAsync(
	'CommandService.createObject passes the caller key and throws the typed code',
	async () => {
		const bodies: Array<Record<string, unknown>> = [];
		const original = globalThis.fetch;
		let reply: unknown = {
			code: 403,
			message: 'collection created',
			data: null,
			error_code: 'policy_rejected',
			details: {}
		};
		globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
			assert(String(input).endsWith(`/api/v1/workspaces/${WS}/flow/objects`), 'endpoint');
			bodies.push(JSON.parse(String(init?.body)));
			return new Response(JSON.stringify(reply), {
				headers: { 'Content-Type': 'application/json' }
			});
		}) as typeof fetch;
		const service = new FlowCommandService();
		const session = new CreateCollectionSession(
			WS,
			service,
			(() => {
				let n = 0;
				return () => `k-${++n}`;
			})()
		);
		let first;
		let second;
		try {
			first = await session.submit(draft());
			reply = { code: 0, message: 'ok', data: accepted('col-9') };
			second = await session.submit(draft());
		} finally {
			globalThis.fetch = original;
		}
		assert(first?.kind === 'failed', 'first failed');
		if (first?.kind === 'failed') {
			assertEqual(first.code, 'policy_rejected', 'typed code, not the HTTP mapping');
			assertEqual(first.key, 'flow.error.policy_rejected', 'key');
		}
		assertEqual(second?.kind, 'created', 'created');
		assertEqual(bodies[0].idempotency_key, 'k-1', 'wire key');
		assertEqual(bodies[1].idempotency_key, 'k-1', 'retry reuses the wire key');
		assertEqual(bodies[0].object_type, 'collection', 'collection');
	}
);

// ---- component wiring (static) --------------------------------------------------------------

suite.check('the navigator wires search, lag badge and Collection create through adapters', () => {
	const nav = readFileSync(join(ROOT, 'src/lib/components/flow/FlowNavigator.svelte'), 'utf8');
	const dialog = readFileSync(
		join(ROOT, 'src/lib/components/flow/CreateCollectionDialog.svelte'),
		'utf8'
	);
	assert(!nav.includes('comingSoon'), 'the disabled placeholder is gone');
	for (const source of [nav, dialog]) {
		assert(
			!/import\s+(?!type\b)[^;]*from '\$lib\/api\/flow'/.test(source),
			'components import only types from $lib/api/flow'
		);
	}
	assert(/role="listbox"/.test(nav) && /role="option"/.test(nav), 'listbox/option');
	assert(nav.includes('aria-activedescendant'), 'aria-activedescendant');
	assert(/data-testid="flow-lag-badge"/.test(nav), 'lag badge');
	const badge = nav.slice(nav.indexOf('<p', nav.indexOf('lagState.kind ===') - 200));
	assert(/aria-live="polite"/.test(badge.slice(0, 400)), 'lag badge is a polite live region');
	assert(nav.includes("o.object_type === 'collection'"), 'collections are listed in the tree');
	assert(dialog.includes('CreateCollectionSession'), 'dialog uses the session');
});

await suite.checkAsync('every new i18n key exists in zh and en', async () => {
	const keys = [
		SEARCH_STALE_FRONTIER_KEY,
		SEARCH_TOO_BROAD_KEY,
		SEARCH_TOO_LONG_KEY,
		SEARCH_GENERIC_ERROR_KEY,
		'flow.search.label',
		'flow.search.placeholder',
		'flow.search.requireCurrent',
		'flow.search.empty',
		'flow.search.stale',
		'flow.search.resultCount',
		'flow.lag.synced',
		'flow.lag.lagging',
		'flow.lag.unavailable',
		'flow.lag.loading',
		'flow.nav.createCollection.menuItem',
		'flow.nav.createCollection.viewName.table',
		'flow.nav.createCollection.viewName.board',
		'flow.nav.createCollection.defaultFieldLabel'
	];
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(ROOT, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		assertDeepEqual(
			keys.filter((key) => typeof lookup(key) !== 'string'),
			[],
			`${locale}.json missing keys`
		);
		assertEqual(lookup('flow.search.comingSoon'), undefined, `${locale}: comingSoon removed`);
	}
});

finish(suite.result());
