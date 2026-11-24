import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { apiClient, type ApiResult } from '../../src/lib/api/client';
import { botsApi } from '../../src/lib/api/bots';
import {
	DEFAULT_LEVELS,
	GRANT_LEVELS,
	WORKSPACE_FORM,
	accessOutcome,
	accessRequest,
	accessSummary,
	formFromAccess,
	levelFor,
	needsWritePermission,
	withGrant,
	withScope,
	type AccessForm
} from '../../src/lib/admin/bot-access/rules';

const ROOT = new URL('../..', import.meta.url).pathname;
const read = (path: string) => readFileSync(join(ROOT, path), 'utf8');

// Fake translation: the key, then the values, so a test sees what was asked for.
const translate = (key: string, options?: { values?: Record<string, unknown> }) =>
	options?.values ? `${key} ${JSON.stringify(options.values)}` : key;

const projectsForm: AccessForm = {
	scope: 'projects',
	defaultLevel: 'comment',
	grants: { p1: 'write', p2: 'read' }
};

test('a workspace-wide token sends no level and no grant', () => {
	assert.deepEqual(accessRequest(WORKSPACE_FORM), {
		project_scope: 'workspace',
		default_level: 'none',
		grants: []
	});
	// switching back to "All projects" clears what was chosen, as the server requires
	assert.deepEqual(accessRequest(withScope(projectsForm, 'workspace')), {
		project_scope: 'workspace',
		default_level: 'none',
		grants: []
	});
});

test('selected projects send the default level and one grant per project, in the order shown', () => {
	assert.deepEqual(accessRequest(projectsForm, ['p2', 'p1', 'p3']), {
		project_scope: 'projects',
		default_level: 'comment',
		grants: [
			{ project_id: 'p2', level: 'read' },
			{ project_id: 'p1', level: 'write' }
		]
	});
});

test('a grant on a project that is not in the loaded list is kept', () => {
	const request = accessRequest(projectsForm, ['p1']);
	assert.deepEqual(
		request.grants.map((grant) => grant.project_id),
		['p1', 'p2']
	);
});

test('setting and clearing a level touches only that project', () => {
	const cleared = withGrant(projectsForm, 'p1', '');
	assert.deepEqual(cleared.grants, { p2: 'read' });
	assert.deepEqual(projectsForm.grants, { p1: 'write', p2: 'read' }, 'the input is not changed');
	assert.deepEqual(withGrant(cleared, 'p3', 'comment').grants, { p2: 'read', p3: 'comment' });
	assert.equal(levelFor(projectsForm, 'p1'), 'write');
	assert.equal(levelFor(projectsForm, 'p9'), 'comment');
});

test('the form reads the answer and falls back to the narrowest meaning', () => {
	assert.deepEqual(
		formFromAccess({
			project_scope: 'projects',
			default_level: 'read',
			grants: [{ project_id: 'p1', level: 'comment' }]
		}),
		{ scope: 'projects', defaultLevel: 'read', grants: { p1: 'comment' } }
	);
	assert.deepEqual(
		formFromAccess({ project_scope: 'workspace', default_level: 'none', grants: [] }),
		{
			scope: 'workspace',
			defaultLevel: 'none',
			grants: {}
		}
	);
	const odd = formFromAccess({
		project_scope: 'projects',
		default_level: 'admin',
		grants: [
			{ project_id: 'p1', level: 'root' },
			{ project_id: 'p2', level: 'read' }
		]
	});
	assert.equal(odd.defaultLevel, 'none');
	assert.deepEqual(odd.grants, { p2: 'read' });
	assert.equal(
		formFromAccess({ project_scope: 'future', default_level: 'read', grants: [] }).scope,
		'workspace'
	);
});

test('the level choices are the ones the server accepts', () => {
	assert.deepEqual([...DEFAULT_LEVELS], ['none', 'read', 'comment']);
	assert.deepEqual([...GRANT_LEVELS], ['read', 'comment', 'write']);
});

test('a comment or write level needs a token with write permission', () => {
	assert.equal(needsWritePermission(projectsForm, ['read']), true);
	assert.equal(needsWritePermission(projectsForm, ['read', 'write']), false);
	assert.equal(needsWritePermission(projectsForm, ['admin']), false);
	// read levels only: nothing to warn about
	assert.equal(
		needsWritePermission({ scope: 'projects', defaultLevel: 'read', grants: { p1: 'read' } }, [
			'read'
		]),
		false
	);
	// only the default level asks for comments
	assert.equal(
		needsWritePermission({ scope: 'projects', defaultLevel: 'comment', grants: {} }, ['read']),
		true
	);
	// a grant alone does
	assert.equal(
		needsWritePermission({ scope: 'projects', defaultLevel: 'none', grants: { p1: 'write' } }, [
			'read'
		]),
		true
	);
	assert.equal(needsWritePermission(WORKSPACE_FORM, ['read']), false);
});

test('a refused save is told apart: managed bot, no right, anything else', () => {
	const result = (code: number): ApiResult<null> => ({ code, message: 'm', data: null });
	assert.equal(accessOutcome(result(409)).kind, 'managed');
	assert.equal(accessOutcome(result(403)).kind, 'forbidden');
	assert.equal(accessOutcome(result(400)).kind, 'failed');
});

test('the list column names the reach of each token', () => {
	assert.equal(
		accessSummary(
			{ project_scope: 'workspace', default_project_level: 'none', grant_count: 0 },
			false,
			translate,
			'en'
		),
		'botAccess.summary.workspace'
	);
	// a token from an older answer without the fields is workspace-wide
	assert.equal(accessSummary({}, false, translate, 'en'), 'botAccess.summary.workspace');
	assert.equal(
		accessSummary(
			{ project_scope: 'projects', default_project_level: 'comment', grant_count: 2 },
			false,
			translate,
			'en'
		),
		'botAccess.summary.projects {"count":"botAccess.summary.grants.other {\\"count\\":2}","level":"botAccess.defaultLevel.comment"}'
	);
	assert.equal(
		accessSummary(
			{ project_scope: 'projects', default_project_level: 'read', grant_count: 1 },
			false,
			translate,
			'en'
		).includes('botAccess.summary.grants.one'),
		true
	);
	// a bot of a project connection says so instead of a reach
	assert.equal(
		accessSummary(
			{ project_scope: 'workspace', default_project_level: 'none', grant_count: 0 },
			true,
			translate,
			'en'
		),
		'botAccess.managed'
	);
});

test('the project-access routes', async () => {
	const calls: Array<{ method: string; url: string; body?: unknown }> = [];
	const client = apiClient as unknown as Record<string, unknown>;
	const reply: ApiResult<unknown> = { code: 0, message: 'ok', data: null };
	const methods = ['get', 'put'];
	for (const method of methods) {
		client[method] = (url: string, body?: unknown) => {
			calls.push({ method, url, body });
			return Promise.resolve(reply);
		};
	}
	try {
		await botsApi.getProjectAccess('w1', 'b1');
		await botsApi.putProjectAccess('w1', 'b1', accessRequest(projectsForm));
	} finally {
		// own properties of the shared client: remove them so the other suites see the real one
		for (const method of methods) delete client[method];
	}
	assert.deepEqual(
		calls.map((call) => `${call.method} ${call.url}`),
		[
			'get /api/v1/workspaces/w1/bots/b1/project-access',
			'put /api/v1/workspaces/w1/bots/b1/project-access'
		]
	);
	assert.equal((calls[1].body as { default_level: string }).default_level, 'comment');
});

test('the members page shows the column and the button, managed bots are locked, and texts exist in en and zh', () => {
	const page = read('src/routes/(app)/workspace/[workspaceId]/members/+page.svelte');
	assert.match(page, /BotAccessDialog/);
	assert.match(page, /accessSummary\(/);
	assert.match(page, /data-testid="bot-access-open"/);
	const dialog = read('src/lib/admin/bot-access/BotAccessDialog.svelte');
	assert.match(dialog, /needsWritePermission\(/, 'the write-permission hint');
	assert.match(dialog, /botAccess\.managed/, 'the managed state');
	for (const lang of ['en', 'zh']) {
		const messages = JSON.parse(read(`src/lib/admin/bot-access/locale.${lang}.json`)) as {
			botAccess: Record<string, unknown>;
		};
		const access = messages.botAccess as Record<string, Record<string, string> | string>;
		for (const key of [
			'title',
			'column',
			'button',
			'saved',
			'managed',
			'scopeWorkspace',
			'scopeProjects',
			'otherProjects',
			'likeOthers',
			'needsWrite'
		])
			assert.ok(access[key], `${lang} botAccess.${key}`);
		for (const level of DEFAULT_LEVELS)
			assert.ok((access.defaultLevel as Record<string, string>)[level], `${lang} default ${level}`);
		for (const level of GRANT_LEVELS)
			assert.ok((access.grantLevel as Record<string, string>)[level], `${lang} grant ${level}`);
	}
});
