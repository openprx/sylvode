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
	SITE_TABS,
	accessOf,
	bindingFields,
	draftOf,
	emptyDraft,
	inFieldOrder,
	isBindingKey,
	issuesAt,
	pagesUsing,
	parseMaxPerHour,
	placeIssues,
	projectionOf,
	withReadField,
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
	assert.equal(tabOf('bindings'), 'bindings');
	assert.deepEqual([...SITE_TABS], ['pages', 'bindings', 'roles', 'settings', 'drafts']);
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

// Form bindings

const outlets = {
	text: { site_read: 'value', site_submit: 'value', filter: 'value', sort: 'value' },
	member: { site_read: 'reject', site_submit: 'reject', filter: 'reject', sort: 'reject' },
	formula: { site_read: 'value', site_submit: 'value', filter: 'value', sort: 'value' },
	file: { site_read: 'reject', site_submit: 'reject', filter: 'reject', sort: 'reject' },
	textarea: { site_read: 'value', site_submit: 'value', filter: 'reject', sort: 'reject' }
};
const schema = {
	fields: [
		{ key: 'name', label: 'Name', type: 'text' },
		{ key: 'note', type: 'textarea' },
		{ key: 'owner', label: 'Owner', type: 'member' },
		{ key: 'total', label: 'Total', type: 'formula', formula: { op: 'sum', args: [] } },
		{ key: 'created_at', label: 'Created', type: 'text' }
	]
};

test('binding keys follow the server rule', () => {
	assert.equal(isBindingKey('requests'), true);
	assert.equal(isBindingKey('a_1'), true);
	assert.equal(isBindingKey('1a'), false);
	assert.equal(isBindingKey('Req'), false);
	assert.equal(isBindingKey(''), false);
	assert.equal(isBindingKey('a'.repeat(41)), false);
	assert.equal(isBindingKey('a'.repeat(40)), true);
});

test('field facts come from the type outlets; derived and opaque types are held back', () => {
	const fields = bindingFields(schema, outlets);
	const by = Object.fromEntries(fields.map((field) => [field.key, field]));
	assert.deepEqual(
		[by.name?.read, by.name?.submit, by.name?.filter, by.name?.sort],
		[true, true, true, true]
	);
	assert.equal(by.note?.label, 'note');
	assert.deepEqual([by.note?.read, by.note?.filter], [true, false]);
	assert.deepEqual([by.owner?.read, by.owner?.submit], [false, false]);
	assert.deepEqual([by.total?.read, by.total?.submit], [true, false]);
	// A form field with a record column's key wins; the other column is appended.
	assert.equal(by.created_at?.column, false);
	assert.deepEqual(
		[by.updated_at?.column, by.updated_at?.read, by.updated_at?.sort, by.updated_at?.filter],
		[true, true, true, false]
	);
	assert.deepEqual(
		bindingFields(null, outlets).map((field) => field.key),
		['created_at', 'updated_at']
	);
});

test('an unknown type grants nothing', () => {
	const [field] = bindingFields({ fields: [{ key: 'x', type: 'mystery' }] }, outlets);
	assert.deepEqual(
		[field?.read, field?.submit, field?.filter, field?.sort],
		[false, false, false, false]
	);
});

test('max per hour is a whole number from 1 to 1000', () => {
	assert.equal(parseMaxPerHour('20'), 20);
	assert.equal(parseMaxPerHour(' 1000 '), 1000);
	assert.equal(parseMaxPerHour('0'), null);
	assert.equal(parseMaxPerHour('1001'), null);
	assert.equal(parseMaxPerHour('2.5'), null);
	assert.equal(parseMaxPerHour(''), null);
	assert.equal(parseMaxPerHour('-3'), null);
});

test('the projection leaves out sections that are off and orders lists by schema', () => {
	const fields = bindingFields(schema, outlets);
	const draft = emptyDraft();
	assert.deepEqual(projectionOf(draft, fields), {});
	draft.submit = { on: true, fields: ['note', 'name'], maxPerHour: '5' };
	draft.visitorRead = {
		scope: 'own',
		fields: ['updated_at', 'name'],
		filter: ['name', 'gone'],
		sort: ['updated_at']
	};
	draft.operatorRead = { scope: 'all', fields: ['name'], filter: [], sort: [] };
	draft.update = { on: true, fields: ['name'] };
	assert.deepEqual(projectionOf(draft, fields), {
		visitor: {
			read: {
				fields: ['name', 'updated_at'],
				scope: 'own',
				filter: ['name'],
				sort: ['updated_at']
			},
			submit: { fields: ['name', 'note'], max_per_hour: 5 }
		},
		operator: {
			read: { fields: ['name'], scope: 'all', filter: [], sort: [] },
			update: { fields: ['name'] }
		}
	});
	draft.submit.on = false;
	draft.visitorRead.scope = 'none';
	const off = projectionOf(draft, fields);
	assert.equal(off.visitor, undefined);
	assert.deepEqual(Object.keys(off), ['operator']);
});

test('a stored binding round-trips through the draft', () => {
	const fields = bindingFields(schema, outlets);
	const binding: SiteBinding = {
		site_id: 's',
		binding_key: 'req',
		form_id: 'f1',
		projection: {
			visitor: {
				read: { fields: ['name'], scope: 'own', filter: ['name'], sort: [] },
				submit: { fields: ['name', 'note'], max_per_hour: 7, captcha: false }
			},
			operator: { update: { fields: ['name'] } }
		}
	};
	const draft = draftOf(binding);
	assert.equal(draft.submit.maxPerHour, '7');
	assert.equal(draft.operatorRead.scope, 'none');
	const sent = projectionOf(draft, fields);
	assert.deepEqual(sent.visitor?.submit, { fields: ['name', 'note'], max_per_hour: 7 });
	assert.deepEqual(sent.operator, { update: { fields: ['name'] } });
});

test('dropping a read field drops its filter and sort', () => {
	const read = { scope: 'own', fields: ['a', 'b'], filter: ['a'], sort: ['a', 'b'] };
	assert.deepEqual(withReadField(read, 'a', false), {
		scope: 'own',
		fields: ['b'],
		filter: [],
		sort: ['b']
	});
	assert.deepEqual(withReadField(read, 'c', true).fields, ['a', 'b', 'c']);
	assert.deepEqual(inFieldOrder(['note', 'zzz', 'name'], bindingFields(schema, outlets)), [
		'name',
		'note',
		'zzz'
	]);
});

test('access counts show what each audience gets', () => {
	assert.deepEqual(accessOf({}), { submit: 0, visitorRead: 0, operatorRead: 0, update: 0 });
	assert.deepEqual(
		accessOf({
			visitor: { read: { fields: ['a', 'b'], scope: 'none' }, submit: { fields: ['a'] } },
			operator: { read: { fields: ['a', 'b', 'c'], scope: 'all' }, update: { fields: ['a'] } }
		}),
		{ submit: 1, visitorRead: 0, operatorRead: 3, update: 1 }
	);
});

test('projection issues are placed at the field the path names', () => {
	const sent = {
		visitor: {
			read: { fields: ['name', 'owner', 'note'], scope: 'own', filter: ['note'], sort: [] },
			submit: { fields: ['name', 'total'], max_per_hour: 5000 }
		},
		operator: { update: { fields: ['owner'] } }
	};
	const placed = placeIssues(
		{
			errors: [
				{ path: '/visitor/read/fields/1', code: 'field_not_allowed' },
				{ path: '/visitor/read/filter/0', code: 'field_not_filterable' },
				{ path: '/visitor/submit/fields/1', code: 'field_not_allowed' },
				{ path: '/visitor/submit/max_per_hour', code: 'limit_invalid' },
				{ path: '/visitor/read/scope', code: 'scope_invalid' },
				{ path: '/operator/update/fields/0', code: 'field_unknown' },
				{ path: '', code: 'shape_invalid' },
				{ path: '/visitor/read/fields/9', code: 'field_unknown' },
				{ code: 'broken' },
				'junk'
			]
		},
		sent
	);
	assert.deepEqual(
		placed.map((issue) => [issue.section, issue.part, issue.key, issue.code]),
		[
			['visitorRead', 'fields', 'owner', 'field_not_allowed'],
			['visitorRead', 'filter', 'note', 'field_not_filterable'],
			['visitorSubmit', 'fields', 'total', 'field_not_allowed'],
			['visitorSubmit', 'limit', null, 'limit_invalid'],
			['visitorRead', 'scope', null, 'scope_invalid'],
			['operatorUpdate', 'fields', 'owner', 'field_unknown'],
			[null, 'fields', null, 'shape_invalid'],
			['visitorRead', 'fields', null, 'field_unknown'],
			[null, 'fields', null, 'broken']
		]
	);
	assert.deepEqual(
		issuesAt(placed, 'visitorRead', 'owner').map((issue) => issue.code),
		['field_not_allowed']
	);
	assert.deepEqual(
		issuesAt(placed, 'visitorRead').map((issue) => issue.code),
		['scope_invalid', 'field_unknown']
	);
	assert.deepEqual(placeIssues(undefined, sent), []);
	assert.deepEqual(placeIssues({ errors: 'x' }, sent), []);
});

test('pages that use a binding key are found from their layouts', () => {
	const pages = [page('p1', '/'), page('p2', '/a'), page('p3', '/b')];
	const keys = { p1: ['req'], p2: ['other'], p3: ['req', 'other'] };
	assert.deepEqual(
		pagesUsing('req', pages, keys).map((p) => p.route),
		['/', '/b']
	);
	assert.deepEqual(pagesUsing('none', pages, keys), []);
	assert.deepEqual(pagesUsing('req', pages, {}), []);
});

test('binding refusal codes are worded by the console', () => {
	assert.equal(knownError('projection_invalid'), 'projection_invalid');
	assert.equal(knownError('binding_key_invalid'), 'binding_key_invalid');
	assert.equal(knownError('binding_key_taken'), 'binding_key_taken');
});
