/**
 * Helpers of the sites console (`lib/project/sites/view.ts`).
 *
 * Run standalone: `bun test tests/features/sites-view.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
	isHttpsOrigin,
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
	installation_id: null
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
	assert.equal(tabOf('pages'), 'settings');
	assert.equal(tabOf(null), 'settings');
});
