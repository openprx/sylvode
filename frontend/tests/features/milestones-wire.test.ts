/**
 * The milestone API client: routes, bodies and the status/paging parameters follow the server
 * contract, and `null` on `target_date` travels (it clears the date).
 *
 * Run standalone: `bun test tests/features/milestones-wire.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import { Suite, assertDeepEqual } from '../support/harness';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { milestonesApi } from '../../src/lib/api/milestones';

const suite = new Suite('milestones-wire');

type Call = { method: string; url: string; body?: unknown };
const calls: Call[] = [];
const reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
const client = apiClient as unknown as Record<string, unknown>;
const METHODS = ['get', 'post', 'put', 'patch', 'delete'];
for (const method of METHODS) {
	client[method] = (url: string, body?: unknown) => {
		calls.push({ method, url, body });
		return Promise.resolve(reply);
	};
}

try {
	await suite.checkAsync('routes and bodies follow the server contract', async () => {
		calls.length = 0;
		await milestonesApi.list('w1');
		await milestonesApi.list('w1', { status: 'closed', page: 2, per_page: 20 });
		await milestonesApi.get('m1');
		await milestonesApi.create('w1', { name: 'Beta', target_date: '2026-12-01' });
		await milestonesApi.update('m1', { status: 'closed' });
		await milestonesApi.update('m1', { target_date: null });
		await milestonesApi.delete('m1');
		assertDeepEqual(
			calls.map((c) => `${c.method} ${c.url}`),
			[
				'get /api/v1/workspaces/w1/milestones',
				'get /api/v1/workspaces/w1/milestones?status=closed&page=2&per_page=20',
				'get /api/v1/milestones/m1',
				'post /api/v1/workspaces/w1/milestones',
				'put /api/v1/milestones/m1',
				'put /api/v1/milestones/m1',
				'delete /api/v1/milestones/m1'
			],
			'routes'
		);
		assertDeepEqual(calls[3].body, { name: 'Beta', target_date: '2026-12-01' }, 'create body');
		assertDeepEqual(calls[4].body, { status: 'closed' }, 'only the changed field is sent');
		nodeAssert.ok(
			Object.prototype.hasOwnProperty.call(calls[5].body, 'target_date') &&
				(calls[5].body as { target_date: unknown }).target_date === null,
			'null clears the target date'
		);
	});
} finally {
	// The patched methods are own properties of the shared client: remove them so that the other
	// suites of this process (they all run in one) see the real client again.
	for (const method of METHODS) delete client[method];
}

test(suite.name, () => {
	const result = suite.result();
	nodeAssert.equal(result.failed, 0, result.failures.join('\n'));
});
