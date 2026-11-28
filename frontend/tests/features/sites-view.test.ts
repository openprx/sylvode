/**
 * Helpers of the sites console (`lib/project/sites/view.ts`).
 *
 * Run standalone: `bun test tests/features/sites-view.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	canEditPages,
	draftOfSite,
	isHttpsOrigin,
	isPackagePage,
	isRoute,
	layoutIssues,
	moveUpdates,
	navFields,
	navProblem,
	navValue,
	pageErrorField,
	pendingBySite,
	roleCandidates,
	isSlug,
	knownError,
	layoutBindingKeys,
	originLines,
	publishSummary,
	settingsBody,
	settingsProblem,
	siteTitle,
	slugFromTitle,
	statusTone,
	tabOf,
	titleFields,
	titleValue,
	validText
} from '../../src/lib/project/sites/view';
import type { SiteBinding, SitePage } from '../../src/lib/project/sites/wire';

const page = (id: string, route: string, audience: SitePage['audience'] = 'visitor'): SitePage => ({
	id,
	site_id: 's',
	route,
	layout_id: `l-${id}`,
	audience,
	nav: null,
	position: 0,
	installation_id: null,
	generation: null
});

const binding = (key: string, projection: SiteBinding['projection']): SiteBinding => ({
	site_id: 's',
	binding_key: key,
	form_id: 'f',
	projection
});

test('titles follow the locale, then en', () => {
	assert.equal(siteTitle('Help', 'zh'), 'Help');
	assert.equal(siteTitle({ en: 'Help', zh: '帮助' }, 'zh-CN'), '帮助');
	assert.equal(siteTitle({ en: 'Help', zh: '帮助' }, 'fr'), 'Help');
	assert.equal(siteTitle({ en: 'Help' }, null), 'Help');
});

test('a title is sent as a string unless a Chinese title is given', () => {
	assert.equal(titleValue(' Help ', ''), 'Help');
	assert.deepEqual(titleValue('Help', ' 帮助 '), { en: 'Help', zh: '帮助' });
	assert.deepEqual(titleFields('Help'), { en: 'Help', zh: '' });
	assert.deepEqual(titleFields({ en: 'Help', zh: '帮助' }), { en: 'Help', zh: '帮助' });
	assert.equal(validText('  '), false);
	assert.equal(validText('x'.repeat(200)), true);
	assert.equal(validText('x'.repeat(201)), false);
});

test('slugs follow the server rule and are proposed from the title', () => {
	for (const ok of ['ab', 'help-desk', '9lives', 'a'.repeat(40)])
		assert.equal(isSlug(ok), true, ok);
	for (const bad of ['a', '-ab', 'Ab', 'a b', 'a_b', 'a'.repeat(41), ''])
		assert.equal(isSlug(bad), false, bad);
	assert.equal(slugFromTitle('Help Desk!'), 'help-desk');
	assert.equal(slugFromTitle('  --Q&A 2026--  '), 'q-a-2026');
	assert.equal(slugFromTitle('帮助'), '');
	assert.equal(slugFromTitle('x'), '');
	assert.equal(isSlug(slugFromTitle('a'.repeat(60))), true);
});

test('an origin is exactly scheme and host', () => {
	assert.equal(isHttpsOrigin('https://help.example.com'), true);
	assert.equal(isHttpsOrigin('https://help.example.com:8443'), true);
	for (const bad of [
		'http://help.example.com',
		'https://help.example.com/',
		'https://help.example.com/path',
		'https://*.example.com',
		'https://user@help.example.com',
		'https://help.example.com?x=1',
		'help.example.com',
		''
	])
		assert.equal(isHttpsOrigin(bad), false, bad);
});

test('settings: lines, problems and the body that keeps the stored theme', () => {
	assert.deepEqual(
		originLines(' https://a.example.com \n\nhttps://a.example.com\nhttps://b.example.com\r\n'),
		['https://a.example.com', 'https://b.example.com']
	);
	assert.equal(settingsProblem('', []), null);
	assert.equal(settingsProblem('https://x.example.com', ['https://y.example.com']), null);
	assert.equal(settingsProblem('http://x.example.com', []), 'allowed_origin_invalid');
	assert.equal(settingsProblem('', ['nope']), 'frame_ancestors_invalid');
	assert.equal(
		settingsProblem(
			'',
			Array.from({ length: 9 }, (_, i) => `https://h${i}.example.com`)
		),
		'frame_ancestors_invalid'
	);
	assert.deepEqual(
		settingsBody({ theme: 'dark', allowed_origin: 'https://old.example.com' }, '', []),
		{
			theme: 'dark'
		}
	);
	assert.deepEqual(settingsBody({}, ' https://x.example.com ', ['https://y.example.com']), {
		allowed_origin: 'https://x.example.com',
		frame_ancestors: ['https://y.example.com']
	});
});

test('layout binding keys come from the data sources', () => {
	const spec = {
		data: {
			a: { source: 'query', form: '$bind.requests' },
			b: { source: 'form', form: 'form:notes' },
			c: { source: 'query', form: 'other' }
		}
	};
	assert.deepEqual(layoutBindingKeys(spec).sort(), ['notes', 'requests']);
	assert.deepEqual(layoutBindingKeys(null), []);
	assert.deepEqual(layoutBindingKeys({}), []);
});

test('the publish summary counts what visitors reach and warns without blocking', () => {
	const empty = publishSummary({ pages: [], bindings: [], pageBindingKeys: {}, url: null });
	assert.deepEqual(
		empty.warnings.map((warning) => warning.code),
		['no_visitor_page', 'no_home_page', 'no_url']
	);
	assert.equal(empty.visitorPages, 0);

	const full = publishSummary({
		pages: [page('1', '/'), page('2', '/requests'), page('3', '/inbox', 'operator')],
		bindings: [
			binding('requests', {
				visitor: { submit: { fields: ['a', 'b'] }, read: { fields: ['a'], scope: 'own' } }
			}),
			binding('internal', { operator: { read: { fields: ['a'], scope: 'all' } } }),
			binding('closed', { visitor: { read: { fields: ['a'], scope: 'none' } } })
		],
		pageBindingKeys: { '2': ['requests', 'ghost'], '3': ['internal'] },
		url: 'https://help.example.com/'
	});
	assert.equal(full.visitorPages, 2);
	assert.deepEqual(full.publicBindings, [{ key: 'requests', submit: 2, read: 1 }]);
	assert.deepEqual(full.warnings, [{ code: 'missing_binding', route: '/requests', key: 'ghost' }]);

	const noHome = publishSummary({
		pages: [page('1', '/faq')],
		bindings: [],
		pageBindingKeys: {},
		url: 'https://x/'
	});
	assert.deepEqual(noHome.warnings, [{ code: 'no_home_page' }]);
});

test('status tones, known error codes and tabs', () => {
	assert.equal(statusTone('draft'), 'neutral');
	assert.equal(statusTone('published'), 'success');
	assert.equal(statusTone('disabled'), 'danger');
	assert.equal(knownError('slug_taken'), 'slug_taken');
	assert.equal(knownError('internal'), null);
	assert.equal(knownError(undefined), null);
	assert.equal(tabOf('settings'), 'settings');
	assert.equal(tabOf('drafts'), 'drafts');
	assert.equal(tabOf('roles'), 'roles');
	assert.equal(tabOf('nonsense'), 'pages');
	assert.equal(tabOf(null), 'pages');
});

test('routes follow the server grammar', () => {
	for (const ok of ['/', '/help', '/a-b/c1', '/requests/:id', '/x/:record_id'])
		assert.equal(isRoute(ok), true, ok);
	for (const bad of [
		'',
		'help',
		'/help/',
		'//',
		'/Help',
		'/:id',
		'/a/:',
		'/a/:Id',
		'/a b',
		'/a/:b1'
	])
		assert.equal(isRoute(bad), false, bad);
	assert.equal(isRoute(`/${'a'.repeat(200)}`), false);
});

test('a menu label is empty, or has an English text', () => {
	assert.equal(navValue(' ', ''), null);
	assert.equal(navValue('Help', ''), 'Help');
	assert.deepEqual(navValue('Help', '帮助'), { en: 'Help', zh: '帮助' });
	assert.deepEqual(navFields(null), { en: '', zh: '' });
	assert.deepEqual(navFields({ en: 'Help', zh: '帮助' }), { en: 'Help', zh: '帮助' });
	assert.equal(navProblem('', ''), false);
	assert.equal(navProblem('', '帮助'), true);
	assert.equal(navProblem('Help', ''), false);
	assert.equal(navProblem('x'.repeat(201), ''), true);
});

test('page errors land under their field', () => {
	assert.equal(pageErrorField('route_taken'), 'route');
	assert.equal(pageErrorField('route_invalid'), 'route');
	assert.equal(pageErrorField('nav_invalid'), 'nav');
	assert.equal(pageErrorField('audience_invalid'), 'audience');
	assert.equal(pageErrorField('layout_invalid'), 'layout');
	assert.equal(pageErrorField('layout_required'), 'layout');
	assert.equal(pageErrorField('package_page_readonly'), null);
	assert.equal(pageErrorField(undefined), null);
	for (const code of ['route_taken', 'site_route_taken', 'package_page_readonly', 'role_invalid'])
		assert.equal(knownError(code), code);
});

test('layout issues list the path and the code', () => {
	assert.deepEqual(
		layoutIssues({ errors: [{ path: '/data/a/form', code: 'binding_unknown' }, { code: 'x' }, 7] }),
		['/data/a/form  binding_unknown', '/  x']
	);
	assert.deepEqual(layoutIssues(undefined), []);
	assert.deepEqual(layoutIssues({ errors: 'no' }), []);
});

test('package pages are told apart and cannot move', () => {
	const own = (id: string, position: number): SitePage => ({ ...page(id, `/${id}`), position });
	const pkg: SitePage = { ...page('pk', '/pk'), installation_id: 'i', generation: 2 };
	assert.equal(isPackagePage(pkg), true);
	assert.equal(isPackagePage(own('a', 0)), false);
	// Equal positions are renumbered so the order exists.
	assert.deepEqual(moveUpdates([own('a', 0), own('b', 0), own('c', 0)], 'b', -1), [
		{ id: 'a', position: 1 },
		{ id: 'c', position: 2 }
	]);
	assert.deepEqual(moveUpdates([own('a', 3), own('b', 4)], 'a', 1), [
		{ id: 'b', position: 3 },
		{ id: 'a', position: 4 }
	]);
	assert.deepEqual(moveUpdates([own('a', 3), own('b', 4)], 'a', -1), []);
	assert.deepEqual(moveUpdates([own('a', 3), own('b', 4)], 'b', 1), []);
	// A package page between two own pages is skipped over.
	assert.deepEqual(moveUpdates([own('a', 0), pkg, own('b', 1)], 'b', -1), [
		{ id: 'b', position: 0 },
		{ id: 'a', position: 1 }
	]);
	assert.deepEqual(moveUpdates([pkg], 'pk', 1), []);
});

test('only admins and site editors change pages', () => {
	assert.equal(canEditPages(true, null), true);
	assert.equal(canEditPages(false, 'site_editor'), true);
	assert.equal(canEditPages(false, 'site_operator'), false);
	assert.equal(canEditPages(false, null), false);
});

test('pending drafts are counted per site', () => {
	const drafts = [
		{ target: { site_id: 's1' } },
		{ target: { site_id: 's1' } },
		{ target: { site_id: 's2' } },
		{ target: {} }
	];
	const counts = pendingBySite(drafts);
	assert.equal(counts.get('s1'), 2);
	assert.equal(counts.get('s2'), 1);
	assert.equal(counts.size, 2);
	assert.equal(draftOfSite(drafts[0] as { target: Record<string, unknown> }, 's1'), true);
	assert.equal(draftOfSite(drafts[2] as { target: Record<string, unknown> }, 's1'), false);
});

test('role candidates are active people without a role', () => {
	const members = [
		{ user_id: 'u1', name: 'A', email: 'a@x' },
		{ user_id: 'u2', name: 'B', email: 'b@x' },
		{ user_id: 'u3', name: 'Bot', email: 'bot@x', entity_type: 'bot' },
		{ user_id: 'u4', name: 'D', email: 'd@x', entity_type: 'human' }
	];
	const roles = [{ site_id: 's', user_id: 'u1', role: 'site_operator' as const }];
	assert.deepEqual(
		roleCandidates(members, roles).map((member) => member.user_id),
		['u2', 'u4']
	);
});
