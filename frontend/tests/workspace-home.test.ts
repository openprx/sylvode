/**
 * Workspace home (`/workspace/{id}`, task FP-N6).
 *
 * Pure decisions in `src/lib/workspace/home.ts`: the Flow / admin card visibility matrix
 * (flag x admin), the five-project cut, the `Sylvode - ` title, the home-path match; the loader
 * against a mocked `globalThis.fetch` (request shape, role and error branching on `code` /
 * `error_code` only); and the app layout's title table, where every page-title key a branch can
 * return must carry the `Sylvode - ` prefix in both locales.
 *
 * Run standalone: `bun tests/workspace-home.test.ts`
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { get } from 'svelte/store';
import { Suite, assert, assertDeepEqual, assertEqual, finish } from './support/harness';
import {
	RECENT_PROJECT_LIMIT,
	WORKSPACE_ADMIN_LINKS,
	defaultWorkspaceHomeDeps,
	isWorkspaceAdminRole,
	isWorkspaceHomePath,
	loadWorkspaceHome,
	recentProjects,
	rememberWorkspaceName,
	workspaceHomeCards,
	workspaceHomeNames,
	workspaceHomeTitle,
	type Translate
} from '../src/lib/workspace/home';
import { createFlowFeatureStore } from '../src/lib/stores/flow-feature';
import type { Project } from '../src/lib/api/projects';

const suite = new Suite('workspace-home');
const root = fileURLToPath(new URL('..', import.meta.url));
const WORKSPACE = '11111111-1111-4111-8111-111111111111';
const ME = '22222222-2222-4222-8222-222222222222';

type Messages = Record<string, unknown>;
const locales: Record<'en' | 'zh', Messages> = {
	en: JSON.parse(readFileSync(join(root, 'src/lib/i18n/en.json'), 'utf8')) as Messages,
	zh: JSON.parse(readFileSync(join(root, 'src/lib/i18n/zh.json'), 'utf8')) as Messages
};

function lookup(messages: Messages, key: string): unknown {
	return key
		.split('.')
		.reduce<unknown>(
			(node, part) => (node as Record<string, unknown> | undefined)?.[part],
			messages
		);
}

/** A translator over the real locale file, with `{name}` interpolation, so the title checks run
 * against the shipped strings rather than a fixture. */
function translator(locale: 'en' | 'zh'): Translate {
	return (key, options) => {
		const value = lookup(locales[locale], key);
		if (typeof value !== 'string') throw new Error(`${locale}: missing key ${key}`);
		return value.replace(/\{(\w+)\}/g, (_, name: string) => options?.values?.[name] ?? `{${name}}`);
	};
}

function project(index: number, updated: string, created = '2026-01-01T00:00:00Z'): Project {
	return {
		id: `p${index}`,
		workspace_id: WORKSPACE,
		name: `Project ${index}`,
		key: `P${index}`,
		type_key: 'code_project',
		type_settings: {},
		created_at: created,
		updated_at: updated
	};
}

// ---------------------------------------------------------------- visibility ---------------

suite.check('card visibility matrix: flag x admin (four combinations)', () => {
	assertDeepEqual(workspaceHomeCards(true, true), { flow: 'open', admin: true }, 'flag on, admin');
	assertDeepEqual(
		workspaceHomeCards(true, false),
		{ flow: 'open', admin: false },
		'flag on, member'
	);
	assertDeepEqual(
		workspaceHomeCards(false, true),
		{ flow: 'enable', admin: true },
		'flag off, admin'
	);
	assertDeepEqual(
		workspaceHomeCards(false, false),
		{ flow: 'hidden', admin: false },
		'flag off, member: no Flow card and no admin card'
	);
});

suite.check(
	'only owner and admin are workspace admins; unknown and missing roles fail closed',
	() => {
		assertEqual(isWorkspaceAdminRole('owner'), true, 'owner');
		assertEqual(isWorkspaceAdminRole('admin'), true, 'admin');
		assertEqual(isWorkspaceAdminRole('member'), false, 'member');
		assertEqual(isWorkspaceAdminRole(null), false, 'null');
		assertEqual(isWorkspaceAdminRole(undefined), false, 'undefined');
		assertEqual(isWorkspaceAdminRole('Owner' as never), false, 'case-sensitive');
	}
);

suite.check('admin card lists Members, Webhooks, Connections, Settings in that order', () => {
	assertDeepEqual(
		WORKSPACE_ADMIN_LINKS.map((link) => [link.key, link.route, link.labelKey]),
		[
			['members', '/(app)/workspace/[workspaceId]/members', 'nav.members'],
			['webhooks', '/(app)/workspace/[workspaceId]/webhooks', 'nav.webhook'],
			['connections', '/(app)/workspace/[workspaceId]/connections', 'nav.connections'],
			['settings', '/(app)/workspace/[workspaceId]/settings', 'nav.workspaceSettings']
		],
		'admin links'
	);
});

await suite.checkAsync(
	'Flow card reads the shared fail-closed flag store: a failed read hides it from members',
	async () => {
		const answers = [
			{ code: 0, message: 'ok', data: { flow_enabled: true } },
			{ code: 403, message: 'flow_enabled: true' },
			{ code: 0, message: 'ok', data: null }
		];
		const store = createFlowFeatureStore(async () => answers.shift() as never);
		await store.refresh(WORKSPACE);
		assertEqual(workspaceHomeCards(get(store)[WORKSPACE] === true, false).flow, 'open', 'enabled');
		await store.refresh(WORKSPACE);
		assertEqual(workspaceHomeCards(get(store)[WORKSPACE] === true, false).flow, 'hidden', '403');
		await store.refresh(WORKSPACE);
		assertEqual(
			workspaceHomeCards(get(store)[WORKSPACE] === true, false).flow,
			'hidden',
			'null data'
		);
		assertEqual(
			workspaceHomeCards(get(store)['never-read'] === true, false).flow,
			'hidden',
			'workspace never read'
		);
	}
);

// ---------------------------------------------------------------- recent projects ---------

suite.check('recent projects: at most five, newest update first, input untouched', () => {
	const input = [
		project(1, '2026-10-01T00:00:00Z'),
		project(2, '2026-10-07T00:00:00Z'),
		project(3, '2026-10-03T00:00:00Z'),
		project(4, '2026-10-06T00:00:00Z'),
		project(5, '2026-10-02T00:00:00Z'),
		project(6, '2026-10-05T00:00:00Z'),
		project(7, '2026-10-04T00:00:00Z')
	];
	const before = input.map((p) => p.id);
	assertEqual(RECENT_PROJECT_LIMIT, 5, 'limit');
	assertDeepEqual(
		recentProjects(input).map((p) => p.id),
		['p2', 'p4', 'p6', 'p7', 'p3'],
		'five most recently updated'
	);
	assertDeepEqual(
		input.map((p) => p.id),
		before,
		'input order unchanged'
	);
	assertDeepEqual(
		recentProjects(input.slice(0, 3)).map((p) => p.id),
		['p2', 'p3', 'p1'],
		'fewer than five'
	);
	assertDeepEqual(recentProjects([]), [], 'empty');
});

suite.check('recent projects: ties on update time fall back to creation time', () => {
	const same = '2026-10-01T00:00:00Z';
	const ordered = recentProjects([
		project(1, same, '2026-01-01T00:00:00Z'),
		project(2, same, '2026-03-01T00:00:00Z'),
		project(3, same, '2026-02-01T00:00:00Z')
	]);
	assertDeepEqual(
		ordered.map((p) => p.id),
		['p2', 'p3', 'p1'],
		'creation tiebreak'
	);
});

// ---------------------------------------------------------------- title -------------------

suite.check('home path matches /workspace/{id} only, never the list or a sub-page', () => {
	assertEqual(isWorkspaceHomePath(`/workspace/${WORKSPACE}`), true, 'home');
	assertEqual(isWorkspaceHomePath(`/workspace/${WORKSPACE}/`), true, 'trailing slash');
	assertEqual(isWorkspaceHomePath('/workspace'), false, 'list');
	assertEqual(isWorkspaceHomePath('/workspace/'), false, 'list with slash');
	for (const sub of ['projects', 'connections', 'settings', 'flow', 'members', 'settings/flow']) {
		assertEqual(isWorkspaceHomePath(`/workspace/${WORKSPACE}/${sub}`), false, sub);
	}
});

for (const locale of ['en', 'zh'] as const) {
	suite.check(
		`${locale}: home title is "Sylvode - <name>", and prefixed before the name loads`,
		() => {
			const translate = translator(locale);
			assertEqual(workspaceHomeTitle(translate, 'Acme Ops'), 'Sylvode - Acme Ops', 'with name');
			assertEqual(workspaceHomeTitle(translate, '  Acme Ops  '), 'Sylvode - Acme Ops', 'trimmed');
			for (const missing of [null, undefined, '', '   ']) {
				const title = workspaceHomeTitle(translate, missing);
				assertEqual(
					title,
					translate('pageTitle.workspaceHome'),
					`fallback for ${JSON.stringify(missing)}`
				);
				assert(title.startsWith('Sylvode - '), `fallback "${title}" lacks the prefix`);
			}
		}
	);
}

suite.check('the layout title table: every pageTitle key it returns carries "Sylvode - "', () => {
	const layout = readFileSync(join(root, 'src/routes/(app)/+layout.svelte'), 'utf8');
	const start = layout.indexOf('const appPageTitle');
	const end = layout.indexOf('const currentWorkspaceId');
	assert(start > 0 && end > start, 'appPageTitle block not found');
	const block = layout.slice(start, end);
	assert(block.includes('isWorkspaceHomePath(pathname)'), 'no workspace home branch');
	assert(
		block.indexOf('isWorkspaceHomePath(pathname)') < block.indexOf('/workspace\\/[^/]+\\/members'),
		'home branch must precede the workspace sub-page branches'
	);
	const keys = [...block.matchAll(/\$t\('(pageTitle\.[A-Za-z]+)'/g)].map((m) => m[1] as string);
	keys.push('pageTitle.workspaceHome', 'pageTitle.workspaceHomeWithName');
	for (const required of [
		'pageTitle.workspaceConnections',
		'pageTitle.flowWorkspace',
		'pageTitle.workspaceWorkflows'
	]) {
		assert(keys.includes(required), `layout has no branch for ${required}`);
	}
	const unprefixed: string[] = [];
	for (const locale of ['en', 'zh'] as const) {
		for (const key of keys) {
			if (key === 'pageTitle.appDefault') continue;
			const value = lookup(locales[locale], key);
			if (typeof value !== 'string' || !value.startsWith('Sylvode - ')) {
				unprefixed.push(`${locale}:${key}=${JSON.stringify(value)}`);
			}
		}
	}
	assertDeepEqual(unprefixed, [], 'title keys without the prefix');
});

suite.check('no workspace sub-page overrides the layout title without the prefix', () => {
	const pages = [
		'src/routes/(app)/workspace/[workspaceId]/connections/+page.svelte',
		'src/routes/(app)/workspace/[workspaceId]/+page.svelte',
		'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]/+page.svelte'
	];
	for (const path of pages) {
		const source = readFileSync(join(root, path), 'utf8');
		for (const match of source.matchAll(/<title>([\s\S]*?)<\/title>/g)) {
			const inner = (match[1] as string).trim();
			assert(/^\{[^}]*\}$/.test(inner), `${path}: <title> carries literal text: ${inner}`);
			const keys = [...source.matchAll(/\$t\('(pageTitle\.[A-Za-z]+)'/g)].map(
				(m) => m[1] as string
			);
			assert(keys.length > 0, `${path}: <title> not built from a pageTitle key`);
			for (const key of keys) {
				for (const locale of ['en', 'zh'] as const) {
					const value = lookup(locales[locale], key);
					assert(
						typeof value === 'string' && value.startsWith('Sylvode - '),
						`${path}: ${locale} ${key} lacks the prefix`
					);
				}
			}
		}
	}
});

suite.check('the layout title follows the name the home page remembered', () => {
	rememberWorkspaceName(WORKSPACE, 'Remembered');
	assertEqual(get(workspaceHomeNames)[WORKSPACE], 'Remembered', 'stored');
	assertEqual(
		workspaceHomeTitle(translator('en'), get(workspaceHomeNames)[WORKSPACE]),
		'Sylvode - Remembered',
		'title'
	);
});

// ---------------------------------------------------------------- loader ------------------

interface Call {
	method: string;
	path: string;
}
type Reply = { code: number; message?: string; data?: unknown; error_code?: string };

let calls: Call[] = [];
let routes: Record<string, Reply> = {};

globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
	const raw = String(input);
	const url = raw.startsWith('http') ? new URL(raw) : new URL(raw, 'http://local');
	const method = init?.method ?? 'GET';
	calls.push({ method, path: url.pathname + url.search });
	const reply = routes[url.pathname] ?? { code: 500, message: 'unrouted' };
	return new Response(JSON.stringify({ message: 'ok', data: null, ...reply }), {
		headers: { 'Content-Type': 'application/json' }
	});
}) as typeof fetch;

const workspaceRow = {
	id: WORKSPACE,
	slug: 'acme',
	name: 'Acme',
	description: 'Ops',
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

function serve(overrides: Record<string, Reply> = {}): void {
	calls = [];
	const projects = Array.from({ length: 7 }, (_, i) =>
		project(i + 1, `2026-10-0${i + 1}T00:00:00Z`)
	);
	routes = {
		[`/api/v1/workspaces/${WORKSPACE}`]: { code: 0, data: workspaceRow },
		[`/api/v1/workspaces/${WORKSPACE}/members`]: {
			code: 0,
			data: {
				items: [
					{ user_id: 'someone-else', workspace_id: WORKSPACE, role: 'owner', joined_at: '' },
					{ user_id: ME, workspace_id: WORKSPACE, role: 'admin', joined_at: '' }
				],
				total: 2,
				page: 1,
				per_page: 20,
				total_pages: 1
			}
		},
		[`/api/v1/workspaces/${WORKSPACE}/projects`]: {
			code: 0,
			data: { items: projects, total: 7, page: 1, per_page: 7, total_pages: 1 }
		},
		'/api/v1/auth/me': { code: 0, data: { user: { id: ME, email: 'me@x', name: 'Me' } } },
		...overrides
	};
}

await suite.checkAsync('loader issues exactly four GETs on the documented paths', async () => {
	serve();
	const outcome = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
	assertEqual(outcome.status, 'ready', 'status');
	assertDeepEqual(
		calls.map((c) => `${c.method} ${c.path}`).sort(),
		[
			'GET /api/v1/auth/me',
			`GET /api/v1/workspaces/${WORKSPACE}`,
			`GET /api/v1/workspaces/${WORKSPACE}/members`,
			`GET /api/v1/workspaces/${WORKSPACE}/projects`
		],
		'requests'
	);
	assert(
		!calls.some((c) => c.path.includes('/features/flow')),
		'the home must not read the flag itself'
	);
	if (outcome.status !== 'ready') return;
	assertEqual(outcome.workspace.name, 'Acme', 'name');
	assertEqual(outcome.workspace.slug, 'acme', 'slug');
	assertEqual(outcome.role, 'admin', 'role of the signed-in user, not the first member');
	assertEqual(outcome.projectTotal, 7, 'total');
	assertDeepEqual(
		outcome.recent.map((p) => p.id),
		['p7', 'p6', 'p5', 'p4', 'p3'],
		'recent five'
	);
	assertEqual(outcome.projectsFailed, false, 'projects ok');
});

await suite.checkAsync(
	'a workspace the viewer cannot read is not_found, whatever the message says',
	async () => {
		for (const reply of [
			{ code: 404, message: 'ok' },
			{ code: 403, message: 'everything is fine' },
			{ code: 200, error_code: 'forbidden', message: 'ok' },
			{ code: 200, error_code: 'not_found', message: 'ok' }
		]) {
			serve({ [`/api/v1/workspaces/${WORKSPACE}`]: reply });
			const outcome = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
			assertEqual(outcome.status, 'not_found', JSON.stringify(reply));
		}
		serve({ [`/api/v1/workspaces/${WORKSPACE}`]: { code: 500, message: 'not found' } });
		assertEqual(
			(await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps)).status,
			'failed',
			'a 500 whose message says "not found" is still a failure'
		);
	}
);

await suite.checkAsync('role fails closed: members or me failing means no role', async () => {
	serve({ [`/api/v1/workspaces/${WORKSPACE}/members`]: { code: 403, message: 'ok' } });
	let outcome = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
	assertEqual(outcome.status === 'ready' ? outcome.role : 'x', null, 'members 403');
	serve({ '/api/v1/auth/me': { code: 500, message: 'ok' } });
	outcome = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
	assertEqual(outcome.status === 'ready' ? outcome.role : 'x', null, 'me 500');
	const rejecting = await loadWorkspaceHome(WORKSPACE, {
		...defaultWorkspaceHomeDeps,
		getMembers: () => Promise.reject(new Error('offline'))
	});
	assertEqual(rejecting.status === 'ready' ? rejecting.role : 'x', null, 'members rejected');
	assertEqual(
		rejecting.status === 'ready'
			? workspaceHomeCards(false, isWorkspaceAdminRole(rejecting.role)).admin
			: true,
		false,
		'no admin card'
	);
});

await suite.checkAsync(
	'a failed project list is reported, not shown as an empty workspace',
	async () => {
		serve({ [`/api/v1/workspaces/${WORKSPACE}/projects`]: { code: 500, message: 'ok' } });
		const outcome = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
		assert(outcome.status === 'ready', 'status');
		assertEqual(outcome.projectsFailed, true, 'projectsFailed');
		assertEqual(outcome.recent.length, 0, 'no rows');
		serve({
			[`/api/v1/workspaces/${WORKSPACE}/projects`]: {
				code: 0,
				data: { items: [], total: 0, page: 1, per_page: 0, total_pages: 0 }
			}
		});
		const empty = await loadWorkspaceHome(WORKSPACE, defaultWorkspaceHomeDeps);
		assert(empty.status === 'ready', 'status');
		assertEqual(empty.projectsFailed, false, 'empty is not failure');
		assertEqual(empty.projectTotal, 0, 'zero');
	}
);

// ---------------------------------------------------------------- i18n --------------------

suite.check('every workspace.home key the page uses exists in both locales', () => {
	const page = readFileSync(
		join(root, 'src/routes/(app)/workspace/[workspaceId]/+page.svelte'),
		'utf8'
	);
	const used = [...page.matchAll(/'(workspace\.home\.[A-Za-z]+)'/g)].map((m) => m[1] as string);
	assert(used.length >= 15, `expected the page to use workspace.home keys, found ${used.length}`);
	for (const locale of ['en', 'zh'] as const) {
		const missing = used.filter((key) => typeof lookup(locales[locale], key) !== 'string');
		assertDeepEqual(missing, [], `${locale} missing`);
		for (const role of ['owner', 'admin', 'member']) {
			assertEqual(
				typeof lookup(locales[locale], `roles.${role}`),
				'string',
				`${locale} roles.${role}`
			);
		}
	}
});

finish(suite.result());
