/**
 * Sidebar workspace role (`src/lib/workspace/role.ts`, FP-R1 R2).
 *
 * The workspace-admin links follow the member role only (owner/admin), as the server does; the
 * instance-admin flag grants nothing. The role is re-read on every workspace change, reads as
 * `null` from the change until the new read lands, and a stale read for a workspace the user
 * already left cannot overwrite the current one. The last check pins the app layout's wiring.
 *
 * Run standalone: `bun tests/workspace-role.test.ts`
 */

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	WorkspaceRoleTracker,
	roleFromMembersResult,
	showWorkspaceAdminLinks,
	type MembersFetcher
} from '../src/lib/workspace/role';
import type { ApiResult, PaginatedData } from '../src/lib/api/client';
import type { WorkspaceMember } from '../src/lib/api/workspaces';
import type { WorkspaceRole } from '../src/lib/workspace/home';

const suite = new Suite('workspace-role');
const ME = '22222222-2222-4222-8222-222222222222';
const WS_A = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa';
const WS_B = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb';

function members(role: WorkspaceRole | null): ApiResult<PaginatedData<WorkspaceMember>> {
	const items = role
		? [
				{ user_id: ME, role } as WorkspaceMember,
				{ user_id: 'other', role: 'owner' } as WorkspaceMember
			]
		: [{ user_id: 'other', role: 'owner' } as WorkspaceMember];
	return {
		code: 0,
		message: 'ok',
		data: { items, total: items.length, page: 1, per_page: 100, total_pages: 1 }
	} as ApiResult<PaginatedData<WorkspaceMember>>;
}

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((r) => (resolve = r));
	return { promise, resolve };
}

suite.check('admin links: member role decides, the instance-admin flag does not', () => {
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: 'owner', instanceAdmin: true }),
		true,
		'owner + instance admin'
	);
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: 'admin', instanceAdmin: false }),
		true,
		'workspace admin, not instance admin'
	);
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: 'member', instanceAdmin: true }),
		false,
		'member + instance admin'
	);
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: null, instanceAdmin: true }),
		false,
		'non-member instance admin'
	);
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: 'member', instanceAdmin: false }),
		false,
		'plain member'
	);
	assertEqual(
		showWorkspaceAdminLinks({ workspaceRole: null, instanceAdmin: false }),
		false,
		'unknown role'
	);
});

suite.check('the role comes from a successful members read only', () => {
	assertEqual(roleFromMembersResult(members('admin'), ME), 'admin', 'found');
	assertEqual(roleFromMembersResult(members(null), ME), null, 'not a member');
	assertEqual(
		roleFromMembersResult({ code: 403, message: 'admin', data: null }, ME),
		null,
		'error envelope'
	);
	assertEqual(roleFromMembersResult({ code: 0, message: 'ok', data: null }, ME), null, 'no data');
	assertEqual(roleFromMembersResult(null, ME), null, 'no result');
});

await suite.checkAsync(
	'a workspace change re-reads the role and hides links until it lands',
	async () => {
		const reported: Array<WorkspaceRole | null> = [];
		const calls: string[] = [];
		const pending = new Map<
			string,
			ReturnType<typeof deferred<ApiResult<PaginatedData<WorkspaceMember>>>>
		>();
		const fetcher: MembersFetcher = (workspaceId) => {
			calls.push(workspaceId);
			const d = deferred<ApiResult<PaginatedData<WorkspaceMember>>>();
			pending.set(workspaceId, d);
			return d.promise;
		};
		const tracker = new WorkspaceRoleTracker(fetcher, (role) => reported.push(role));

		// Entered from `/workspace`: no workspace in the URL, nothing to read.
		await tracker.select(null, ME);
		assertDeepEqual(calls, [], 'the list page reads no role');
		assertDeepEqual(reported, [null], 'and reports none');

		// Client-side navigation into A (no reload): the role is read right away.
		const a = tracker.select(WS_A, ME);
		assertDeepEqual(calls, [WS_A], 'A read on entry');
		pending.get(WS_A)!.resolve(members('admin'));
		assertEqual(await a, 'admin', 'A role');
		assertDeepEqual(reported, [null, null, 'admin'], 'null while reading, then admin');

		// Switch to B: null immediately (fail closed), then B's own role.
		const b = tracker.select(WS_B, ME);
		assertEqual(
			reported[reported.length - 1],
			null,
			'links hidden as soon as the workspace changes'
		);
		assertDeepEqual(calls, [WS_A, WS_B], 'B read');
		pending.get(WS_B)!.resolve(members('member'));
		assertEqual(await b, 'member', 'B role');
		assertEqual(reported[reported.length - 1], 'member', 'B role reported, not A');

		// Same workspace and user again: no extra read, no flicker.
		const before = reported.length;
		await tracker.select(WS_B, ME);
		assertEqual(calls.length, 2, 'no re-read for the same pair');
		assertEqual(reported.length, before, 'no flicker for the same pair');
	}
);

await suite.checkAsync('a slow read for a workspace already left cannot win', async () => {
	const reported: Array<WorkspaceRole | null> = [];
	const slowA = deferred<ApiResult<PaginatedData<WorkspaceMember>>>();
	const fetcher: MembersFetcher = (workspaceId) =>
		workspaceId === WS_A ? slowA.promise : Promise.resolve(members('member'));
	const tracker = new WorkspaceRoleTracker(fetcher, (role) => reported.push(role));
	const a = tracker.select(WS_A, ME);
	await tracker.select(WS_B, ME);
	slowA.resolve(members('owner'));
	assertEqual(await a, null, 'stale read returns nothing');
	assertEqual(reported[reported.length - 1], 'member', 'B role stands');
	assert(!reported.includes('owner'), 'A role never reported');
});

await suite.checkAsync('a failed or thrown read leaves the links hidden', async () => {
	const reported: Array<WorkspaceRole | null> = [];
	const tracker = new WorkspaceRoleTracker(
		async () => {
			throw new Error('network');
		},
		(role) => reported.push(role)
	);
	assertEqual(await tracker.select(WS_A, ME), null, 'thrown read');
	assertDeepEqual(reported, [null, null], 'only null reported');
	const signedOut = new WorkspaceRoleTracker(
		async () => members('owner'),
		(role) => reported.push(role)
	);
	assertEqual(await signedOut.select(WS_A, null), null, 'no user, no role');
});

suite.check(
	'the app layout re-reads the role on workspace change and drops the instance-admin grant',
	() => {
		const root = fileURLToPath(new URL('..', import.meta.url));
		const layout = readFileSync(join(root, 'src/routes/(app)/+layout.svelte'), 'utf8');
		assert(
			/\$effect\(\(\) => \{\s*void roleTracker\.select\(currentWorkspaceId, currentUserId\);\s*\}\);/.test(
				layout
			),
			'an $effect re-selects the role from currentWorkspaceId'
		);
		assert(
			/showWorkspaceAdminLinks\(/.test(layout),
			'isWorkspaceAdmin goes through showWorkspaceAdminLinks'
		);
		assert(!/workspaceRole === 'admin' \|\| isAdmin/.test(layout), 'no instance-admin grant');
		assert(!/getMembers\(currentWorkspaceId\)/.test(layout), 'no one-shot onMount role read');
	}
);

finish(suite.result());
