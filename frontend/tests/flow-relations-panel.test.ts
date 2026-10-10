/**
 * Relations panel (`rest-api-v1.md` `GET /flow/objects/{object_id}/relations` + `RelationView`;
 * `ui-surface-v1.md` v0.5 "lazy-loaded Relations 区"; `versions/v0.5-collaboration.md`
 * "跨 workspace relation 作为 integrity failure 整次 fail closed").
 *
 * Checks the real `flowApi.getRelations` query against a mocked `globalThis.fetch`, then drives
 * `FlowRelationsController` with an injected read: cursor paging, filter reset, unavailable rows
 * that carry no identifier (so no link and no copy), and fail-closed integrity for a
 * cross-workspace relation, a malformed row, and a rejected read -- including after earlier
 * pages were already shown. Errors branch on `error_code` only.
 *
 * Run standalone: `bun tests/flow-relations-panel.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	FlowRelationsController,
	RELATION_DIRECTION_KEYS,
	RELATION_INTEGRITY_KEY,
	parseRelationRow,
	relationTarget
} from '../src/lib/flow/relations-panel';
import {
	flowApi,
	type FlowRelationListResponse,
	type FlowRelationView,
	type FlowRelationsQuery
} from '../src/lib/api/flow';
import type { ApiResult } from '../src/lib/api/client';

const OBJECT = 'obj-root';
const WS = 'ws-1';

function ok<T>(data: T): ApiResult<T> {
	return { code: 0, message: 'ok', data };
}

function visible(n: number, extra: Record<string, unknown> = {}): FlowRelationView {
	return {
		visibility: 'visible',
		relation_id: `rel-${n}`,
		relation_type: n % 2 === 0 ? 'related' : 'blocks',
		direction: 'outgoing',
		properties: {},
		created_at: '2026-10-10T00:00:00Z',
		other_object: {
			id: `target-${n}`,
			object_type: 'page',
			title: `Target ${n}`,
			lifecycle_status: 'active'
		},
		...extra
	} as FlowRelationView;
}

function harness(replies: Array<ApiResult<FlowRelationListResponse>>) {
	const queries: FlowRelationsQuery[] = [];
	const controller = new FlowRelationsController(OBJECT, WS, () => {}, {
		reads: {
			getRelations: async (objectId, query = {}) => {
				assertEqual(objectId, OBJECT, 'root object id');
				queries.push({ ...query });
				if (queries.length > 20) return { code: 500, message: 'runaway', data: null };
				return replies.length > 1
					? (replies.shift() as ApiResult<FlowRelationListResponse>)
					: replies[0];
			}
		},
		pageSize: 2
	});
	return { controller, queries };
}

const suite = new Suite('flow-relations-panel');

await suite.checkAsync('flowApi.getRelations sends the contract query', async () => {
	const seen: string[] = [];
	const original = globalThis.fetch;
	globalThis.fetch = (async (input: string | URL | Request) => {
		const raw = String(input);
		seen.push(raw.startsWith('http') ? new URL(raw).pathname + new URL(raw).search : raw);
		return new Response(JSON.stringify({ code: 0, message: 'ok', data: { items: [] } }), {
			headers: { 'Content-Type': 'application/json' }
		});
	}) as typeof fetch;
	try {
		await flowApi.getRelations(OBJECT, {
			direction: 'incoming',
			relation_type: 'related',
			cursor: 'c1',
			limit: 50
		});
		await flowApi.getRelations(OBJECT);
	} finally {
		globalThis.fetch = original;
	}
	assertDeepEqual(
		seen,
		[
			`/api/v1/flow/objects/${OBJECT}/relations?direction=incoming&relation_type=related&cursor=c1&limit=50`,
			`/api/v1/flow/objects/${OBJECT}/relations`
		],
		'query string'
	);
});

await suite.checkAsync('pages are loaded lazily with the opaque cursor and appended', async () => {
	const h = harness([
		ok({ items: [visible(1), visible(2)], next_cursor: 'opaque-1' }),
		ok({ items: [visible(3)] })
	]);
	assertEqual(h.queries.length, 0, 'nothing is read before load()');
	await h.controller.load();
	assertDeepEqual(
		h.queries[0],
		{ direction: 'both', limit: 2 },
		'first page: default direction, page size'
	);
	assertEqual(h.controller.snapshot.hasMore, true, 'a next_cursor means more');
	await h.controller.load();
	assertEqual(h.queries.length, 1, 'load() again does not re-read');
	await h.controller.loadMore();
	assertDeepEqual(
		h.queries[1],
		{ direction: 'both', cursor: 'opaque-1', limit: 2 },
		'second page uses the cursor'
	);
	const snap = h.controller.snapshot;
	assertDeepEqual(
		snap.rows.map((row) => relationTarget(row)),
		['target-1', 'target-2', 'target-3'],
		'rows appended in order'
	);
	assertEqual(snap.hasMore, false, 'no cursor, no more');
	await h.controller.loadMore();
	assertEqual(h.queries.length, 2, 'loadMore without a cursor sends nothing');
	assertDeepEqual(snap.relationTypes, ['blocks', 'related'], 'types collected for the filter');
});

await suite.checkAsync('a filter change starts over from the first page', async () => {
	const h = harness([ok({ items: [visible(1)], next_cursor: 'c' }), ok({ items: [visible(2)] })]);
	await h.controller.load();
	await h.controller.setFilter({ direction: 'outgoing', relationType: 'related' });
	assertDeepEqual(
		h.queries[1],
		{ direction: 'outgoing', relation_type: 'related', limit: 2 },
		'filtered query, no cursor'
	);
	assertDeepEqual(
		h.controller.snapshot.rows.map((row) => relationTarget(row)),
		['target-2'],
		'old rows are gone'
	);
});

await suite.checkAsync('an unavailable row carries no identifier: no link, no copy', async () => {
	const leaky = {
		visibility: 'unavailable',
		relation_id: 'secret-rel',
		other_object: { id: 'secret-target' }
	};
	const h = harness([
		ok({ items: [visible(1), leaky as unknown as FlowRelationView, { visibility: 'unavailable' }] })
	]);
	await h.controller.load();
	const rows = h.controller.snapshot.rows;
	assertEqual(rows.length, 3, 'placeholders are kept in place');
	for (const row of rows.slice(1)) {
		assertDeepEqual(row, { kind: 'unavailable' }, 'an unavailable row is exactly { kind }');
		assertEqual(relationTarget(row), null, 'no navigation target');
	}
	assert(!JSON.stringify(rows).includes('secret'), 'nothing from a leaky unavailable row survives');
	assertEqual(relationTarget(rows[0]), 'target-1', 'a visible row links to its target');
});

await suite.checkAsync('cross-workspace relation: the whole panel fails closed', async () => {
	const foreign = visible(9, {
		other_object: {
			id: 't',
			object_type: 'page',
			title: 'Foreign',
			lifecycle_status: 'active',
			workspace_id: 'ws-other'
		}
	});
	const h = harness([
		ok({ items: [visible(1)], next_cursor: 'c' }),
		ok({ items: [visible(2), foreign] })
	]);
	await h.controller.load();
	assertEqual(h.controller.snapshot.rows.length, 1, 'first page shown');
	await h.controller.loadMore();
	const snap = h.controller.snapshot;
	assertDeepEqual(snap.rows, [], 'every row is dropped, including earlier pages');
	assertEqual(snap.failureKey, RELATION_INTEGRITY_KEY, 'integrity error shown');
	assertEqual(snap.hasMore, false, 'no further paging');

	const same = parseRelationRow(
		visible(3, {
			other_object: {
				id: 't',
				object_type: 'page',
				title: 'x',
				lifecycle_status: 'a',
				workspace_id: WS
			}
		}),
		WS
	);
	assertEqual(same.kind, 'visible', 'a row naming the root workspace is fine');
});

await suite.checkAsync(
	'server integrity rejection and malformed rows also fail closed',
	async () => {
		const rejected = harness([
			ok({ items: [visible(1)], next_cursor: 'c' }),
			{
				code: 400,
				message: 'cross-workspace relation integrity violation',
				data: null,
				error_code: 'invalid_update'
			}
		]);
		await rejected.controller.load();
		await rejected.controller.loadMore();
		assertDeepEqual(
			[rejected.controller.snapshot.rows.length, rejected.controller.snapshot.failureKey],
			[0, RELATION_INTEGRITY_KEY],
			'server invalid_update: rows dropped, integrity copy'
		);
		for (const bad of [
			{ visibility: 'secret' },
			{ visibility: 'visible', relation_id: 'r' },
			null
		]) {
			const h = harness([ok({ items: [visible(1), bad as unknown as FlowRelationView] })]);
			await h.controller.load();
			assertDeepEqual(
				[h.controller.snapshot.rows.length, h.controller.snapshot.failureKey],
				[0, RELATION_INTEGRITY_KEY],
				`malformed row ${JSON.stringify(bad)} fails closed`
			);
		}
	}
);

await suite.checkAsync('errors branch on error_code only; retry starts over', async () => {
	const h = harness([
		{ code: 403, message: 'invalid_update integrity', data: null, error_code: 'forbidden' },
		ok({ items: [visible(1)] })
	]);
	await h.controller.load();
	assertEqual(h.controller.snapshot.failureKey, 'flow.error.forbidden', 'mapped from error_code');
	await h.controller.reload();
	assertDeepEqual(
		[h.controller.snapshot.failureKey, h.controller.snapshot.rows.length],
		[null, 1],
		'reload clears the failure'
	);
	const legacy = harness([{ code: 500, message: 'x', data: null }]);
	await legacy.controller.load();
	assertEqual(
		legacy.controller.snapshot.failureKey,
		'flow.relations.error.unavailable',
		'legacy fallback'
	);
});

await suite.checkAsync('every relations i18n key exists in zh and en', async () => {
	const root = new URL('..', import.meta.url).pathname;
	for (const locale of ['zh', 'en']) {
		const data = JSON.parse(readFileSync(join(root, 'src/lib/i18n', `${locale}.json`), 'utf8'));
		const lookup = (key: string): unknown =>
			key
				.split('.')
				.reduce<unknown>((node, part) => (node as Record<string, unknown>)?.[part], data);
		const missing = [
			...Object.values(RELATION_DIRECTION_KEYS),
			RELATION_INTEGRITY_KEY,
			'flow.relations.error.unavailable'
		].filter((key) => typeof lookup(key) !== 'string');
		assertDeepEqual(missing, [], `${locale}.json missing keys`);
	}
});

finish(suite.result());
