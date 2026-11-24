/**
 * The workspace page feed (`GET /my/activities`) carries the people its entries name, and the
 * page shows an assignment by the person's name.
 *
 * Run standalone: `bun test tests/features/my-activity-people.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { installSvelteSsr } from '../support/svelte-ssr';

// `my.ts` words its failures through `svelte-i18n`; the stand-in module is installed first.
await installSvelteSsr();
const { apiClient } = await import('../../src/lib/api/client');
const { myApi } = await import('../../src/lib/api/my');

const ada = '11111111-1111-4111-8111-111111111111';
const bea = '22222222-2222-4222-8222-222222222222';

test('the feed answer keeps the people object by user id', async () => {
	const client = apiClient as unknown as { get: (url: string) => Promise<unknown> };
	const realGet = client.get;
	let asked = '';
	client.get = (url: string) => {
		asked = url;
		return Promise.resolve({
			code: 0,
			message: 'ok',
			data: {
				items: [
					{
						id: 'a1',
						issue_id: 'i1',
						action: 'assigned',
						detail: { from: ada, to: bea },
						user_id: ada
					}
				],
				total: 1,
				page: 1,
				per_page: 10,
				total_pages: 1,
				people: {
					[ada]: { id: ada, name: 'Ada', is_bot: false, state: 'active' },
					[bea]: { id: bea, name: null, is_bot: false, state: 'removed' }
				}
			}
		});
	};
	try {
		const answer = await myApi.getMyActivities({ page: 1, per_page: 10 });
		assert.equal(asked, '/api/v1/my/activities?page=1&per_page=10');
		assert.equal(answer.data?.people[ada]?.name, 'Ada');
		assert.equal(answer.data?.people[bea]?.state, 'removed');
		assert.equal(answer.data?.items[0]?.detail.to, bea);
	} finally {
		client.get = realGet;
	}
});

test('without people in the answer the feed has an empty object, never an id', async () => {
	const client = apiClient as unknown as { get: (url: string) => Promise<unknown> };
	const realGet = client.get;
	client.get = () =>
		Promise.resolve({
			code: 0,
			message: 'ok',
			data: { items: [], total: 0, page: 1, per_page: 10, total_pages: 0 }
		});
	try {
		const answer = await myApi.getMyActivities();
		assert.deepEqual(answer.data?.people, {});
	} finally {
		client.get = realGet;
	}
});

test('the workspace page words an assignment through the people of the answer', () => {
	const page = readFileSync(
		new URL('../../src/routes/(app)/workspace/+page.svelte', import.meta.url),
		'utf8'
	);
	assert.match(page, /personLabelById\(activityPeople, to, get\(t\)\)/);
	assert.ok(!page.includes('getMemberDisplay'));
});

test('the workspace and proposal pages show a work item by its identifier, never by its id', async () => {
	const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');
	assert.match(
		read('../../src/routes/(app)/workspace/+page.svelte'),
		/\{activity\.identifier\} · \{activity\.issue_title\}/
	);
	assert.match(
		read('../../src/routes/(app)/proposals/[id]/+page.svelte'),
		/\{link\.identifier\} · \{link\.issue_title\}/
	);
	const client = apiClient as unknown as { get: (url: string) => Promise<unknown> };
	const realGet = client.get;
	client.get = () =>
		Promise.resolve({
			code: 0,
			message: 'ok',
			data: {
				items: [{ id: 'a1', issue_id: 'i1', identifier: 'QLB-7', action: 'created', detail: {} }],
				total: 1,
				page: 1,
				per_page: 10,
				total_pages: 1
			}
		});
	try {
		const answer = await myApi.getMyActivities();
		assert.equal(answer.data?.items[0]?.identifier, 'QLB-7');
	} finally {
		client.get = realGet;
	}
});
