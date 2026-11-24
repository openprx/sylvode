/**
 * Linked pages of a work item: the API routes and bodies, the states of a link (restricted, in
 * trash, no title), the picker candidates, the page side's counts and the activity wording.
 *
 * Run standalone: `bun test tests/features/page-links.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import type { FlowSearchHit } from '../../src/lib/api/flow';
import { pageLinksApi, type PageLink } from '../../src/lib/api/page-links';
import {
	MAX_PAGE_LINKS,
	canLinkMore,
	pageActivityText,
	pageLinkView,
	pickerCandidates,
	workItemsSummary
} from '../../src/lib/project/page-links/rules';
import { randomUuid } from '../../src/lib/utils/uuid';

type Call = { method: string; url: string; body?: unknown };

function translator(language: 'en' | 'zh') {
	const raw = JSON.parse(
		readFileSync(
			new URL(`../../src/lib/project/page-links/locale.${language}.json`, import.meta.url),
			'utf8'
		)
	) as Record<string, unknown>;
	return (key: string, options?: { values?: Record<string, unknown> }): string => {
		let node: unknown = raw;
		for (const part of key.split('.')) node = (node as Record<string, unknown> | undefined)?.[part];
		assert.equal(typeof node, 'string', `missing text ${key}`);
		return (node as string).replace(/\{(\w+)\}/g, (_, name) => String(options?.values?.[name]));
	};
}

const link = (over: Partial<PageLink> = {}): PageLink => ({
	object_id: randomUuid(),
	title: 'Roadmap',
	access: 'visible',
	lifecycle: 'active',
	linked_at: '2026-11-24T10:00:00Z',
	...over
});

test('routes and bodies follow the server contract', async () => {
	const calls: Call[] = [];
	const reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
	const client = apiClient as unknown as Record<string, unknown>;
	const methods = ['get', 'post', 'delete'];
	const original = new Map(methods.map((m) => [m, Object.getOwnPropertyDescriptor(client, m)]));
	for (const method of methods) {
		client[method] = (url: string, body?: unknown) => {
			calls.push({ method, url, body });
			return Promise.resolve(reply);
		};
	}
	try {
		await pageLinksApi.list('i1');
		await pageLinksApi.link('i1', 'o1');
		await pageLinksApi.unlink('i1', 'o1');
		await pageLinksApi.workItemsOf('o1');
	} finally {
		for (const method of methods) {
			const descriptor = original.get(method);
			if (descriptor) Object.defineProperty(client, method, descriptor);
			else delete client[method];
		}
	}
	assert.deepEqual(
		calls.map((c) => `${c.method} ${c.url}`),
		[
			'get /api/v1/issues/i1/pages',
			'post /api/v1/issues/i1/pages',
			'delete /api/v1/issues/i1/pages/o1',
			'get /api/v2/flow/objects/o1/work-items'
		]
	);
	assert.deepEqual(calls[1].body, { object_id: 'o1' });
});

test('a restricted link shows the state text and never a title', () => {
	const view = pageLinkView(link({ title: null, access: 'restricted' }));
	assert.equal(view.title, null);
	assert.equal(view.stateKey, 'pageLinks.restricted');
	assert.equal(view.restricted, true);
	assert.equal(translator('en')('pageLinks.restricted'), 'Restricted page');
	assert.equal(translator('zh')('pageLinks.restricted'), '受限页面');
});

test('a visible link in the trash keeps its title and is flagged', () => {
	const view = pageLinkView(link({ lifecycle: 'archived' }));
	assert.equal(view.title, 'Roadmap');
	assert.equal(view.stateKey, null);
	assert.equal(view.inTrash, true);
	assert.equal(pageLinkView(link()).inTrash, false);
});

test('a title is never invented: a visible link without one gets the generic word', () => {
	const view = pageLinkView(link({ title: '  ' }));
	assert.equal(view.title, null);
	assert.equal(view.stateKey, 'pageLinks.untitled');
});

test('the 50-link limit is known before the server refuses', () => {
	assert.equal(MAX_PAGE_LINKS, 50);
	assert.equal(canLinkMore(49), true);
	assert.equal(canLinkMore(50), false);
	assert.match(translator('en')('pageLinks.limitReached', { values: { max: 50 } }), /\(50\)/);
});

const hit = (id: string, over: Partial<FlowSearchHit['object']> = {}): FlowSearchHit => ({
	object: { id, object_type: 'page', title: `Page ${id}`, lifecycle_status: 'active', ...over },
	matched_fields: ['title'],
	snippets: {},
	indexed_seq: 1,
	head_seq: 1,
	projection_lag: 0,
	stale: false
});

test('the picker offers active, unlinked pages once each', () => {
	const found = pickerCandidates(
		[
			hit('a'),
			hit('a'),
			hit('b', { lifecycle_status: 'archived' }),
			hit('c', { object_type: 'navigator' }),
			hit('d')
		],
		new Set(['d'])
	);
	assert.deepEqual(found, [{ id: 'a', title: 'Page a' }]);
});

test('the page side counts what it cannot show', () => {
	const item = {
		id: 'i',
		identifier: 'WEB-1',
		title: 'T',
		state: 'todo',
		project_id: 'p'
	};
	assert.deepEqual(workItemsSummary({ items: [], hidden_count: 0, truncated: false }), {
		hiddenCount: 0,
		truncated: false,
		empty: true
	});
	assert.deepEqual(workItemsSummary({ items: [], hidden_count: 3, truncated: false }), {
		hiddenCount: 3,
		truncated: false,
		empty: false
	});
	assert.equal(
		workItemsSummary({ items: [item], hidden_count: 0, truncated: true }).truncated,
		true
	);
	assert.equal(
		translator('en')('pageLinks.workItems.hidden', { values: { count: 3 } }),
		'3 more you cannot see'
	);
});

test('activity entries name the page, or say restricted or removed, and never show an id', () => {
	const id = randomUuid();
	for (const language of ['en', 'zh'] as const) {
		const tr = translator(language);
		const cases: Array<[string, Record<string, unknown>]> = [
			[
				'page_linked',
				{ object_id: id, page: { title: 'Roadmap', access: 'visible', lifecycle: 'active' } }
			],
			[
				'page_linked',
				{ object_id: id, page: { title: null, access: 'restricted', lifecycle: 'active' } }
			],
			[
				'page_unlinked',
				{ object_id: id, page: { title: null, access: 'removed', lifecycle: null } }
			],
			['page_unlinked', { object_id: id }]
		];
		for (const [action, detail] of cases) {
			const text = pageActivityText(tr, action, detail);
			assert.ok(text && text.length > 0);
			assert.ok(!text.includes(id), `${language}: id leaked into "${text}"`);
		}
	}
	const en = translator('en');
	assert.equal(
		pageActivityText(en, 'page_linked', { page: { title: 'Roadmap', access: 'visible' } }),
		'linked the page "Roadmap"'
	);
	assert.equal(
		pageActivityText(en, 'page_linked', { page: { title: null, access: 'restricted' } }),
		'linked a restricted page'
	);
	assert.equal(
		pageActivityText(en, 'page_unlinked', { page: { title: null, access: 'removed' } }),
		'unlinked a page that has since been removed'
	);
	assert.equal(pageActivityText(en, 'comment_added', {}), null);
});
