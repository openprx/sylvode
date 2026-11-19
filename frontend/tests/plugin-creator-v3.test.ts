import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import type { InstalledPluginPackage, PluginManifestV3 } from '../src/lib/api/plugin-packages';
import { creatorTemplates, validateManifestV3 } from '../src/lib/plugins/creator/registry';
import { defaultDraft, buildCreatorPackage } from '../src/lib/plugins/creator/template';
import {
	buildUiViewPackage,
	createMockTransport,
	defaultUiViewDraft,
	invalidUiViewFields,
	readUiViewDraft,
	renderUiViewHtml,
	uiViewContributions,
	type UiViewDraft
} from '../src/lib/plugins/creator/ui-view-template';
import {
	grantsValidV3,
	installedLayoutDraftV3,
	layoutDraftV3,
	toggleGrantV3
} from '../src/lib/plugins/layout-v3';
import { readPluginUpload } from '../src/lib/plugins/upload';

const origin = { slot: 'project.view' as const, formKey: null };
const base = defaultUiViewDraft(origin);
const all = (draft: UiViewDraft): UiViewDraft => ({
	...draft,
	contributions: draft.contributions.map((item) => ({ ...item, enabled: true }))
});
const file = (value: unknown) => new File([JSON.stringify(value)], 'p.json');

test('ui_view origin slot preselects the launching location for all four slots', () => {
	for (const slot of creatorTemplates.ui_view.slots) {
		const draft = defaultUiViewDraft({ slot, formKey: 'invoice_line' });
		assert.deepEqual(
			draft.contributions.filter((item) => item.enabled).map((item) => item.slot),
			[slot]
		);
		assert.equal(draft.readFormKey, 'invoice_line');
		assert.deepEqual(invalidUiViewFields(draft), []);
	}
});

test('ui_view invalid reports each bad field', () => {
	assert.deepEqual(invalidUiViewFields(base), []);
	const bad = (patch: Partial<UiViewDraft>) => invalidUiViewFields({ ...base, ...patch });
	assert.deepEqual(bad({ key: 'Bad-Key' }), ['key']);
	assert.deepEqual(bad({ key: 'library' }), ['key']);
	assert.deepEqual(bad({ name: ' ' }), ['name']);
	assert.deepEqual(bad({ publisher: 'a\u0000b' }), ['publisher']);
	assert.deepEqual(bad({ version: 'v1' }), ['version']);
	assert.deepEqual(bad({ readFormKey: '/x' }), ['readFormKey']);
	assert.deepEqual(bad({ readFields: '' }), ['readFields']);
	assert.deepEqual(bad({ readFields: 'a, a' }), ['readFields']);
	assert.deepEqual(bad({ readFields: Array.from({ length: 33 }, (_, i) => `f${i}`).join(',') }), [
		'readFields'
	]);
	assert.deepEqual(bad({ title: '' }), ['title']);
	assert.deepEqual(bad({ text: 'x'.repeat(321) }), ['text']);
	assert.deepEqual(bad({ presentation: 'chart' as 'table' }), ['presentation']);
	assert.deepEqual(
		bad({ contributions: base.contributions.map((item) => ({ ...item, enabled: false })) }),
		['contributions']
	);
	const edit = (slot: string, patch: object) =>
		bad({
			contributions: base.contributions.map((item) =>
				item.slot === slot ? { ...item, enabled: true, ...patch } : item
			)
		});
	assert.deepEqual(edit('project.view', { label: '' }), ['contributions.project.view.label']);
	assert.deepEqual(edit('project.view', { route: 'A/b' }), ['contributions.project.view.route']);
	assert.deepEqual(edit('form.list.toolbar', { formKey: '' }), [
		'contributions.form.list.toolbar.formKey'
	]);
});

test('ui_view build output satisfies the mirrored v3 rules for every slot combination', async () => {
	const built = await buildUiViewPackage(all(base));
	const manifest = built.upload.manifest;
	assert.equal(validateManifestV3(manifest), null);
	assert.equal(manifest.profile, 'ui');
	assert.equal(manifest.wasm, null);
	assert.deepEqual(manifest.capabilities, []);
	assert.equal('ui' in manifest, false);
	assert.equal('wasm_base64' in built.upload, false);
	assert.deepEqual(
		manifest.contributions.map((item) => [item.slot, item.kind, item.route, item.form_key]),
		[
			['project.view', 'view', 'overview', null],
			['form.list.toolbar', 'action', null, 'order_line'],
			['form.record.detail', 'panel', null, 'order_line'],
			['form.record.actions', 'action', null, 'order_line']
		]
	);
	assert.equal(built.json, (await buildUiViewPackage(all(base))).json);
	for (const slot of creatorTemplates.ui_view.slots) {
		const single = await buildUiViewPackage(defaultUiViewDraft({ slot, formKey: null }));
		assert.equal(validateManifestV3(single.upload.manifest), null);
	}
});

test('ui_view html digest and size match the manifest entry', async () => {
	const { upload, html, json } = await buildUiViewPackage(base);
	const bytes = Buffer.from(upload.web_base64, 'base64');
	assert.equal(bytes.toString('utf8'), html);
	assert.equal(upload.manifest.entry.size, bytes.length);
	assert.equal(upload.manifest.entry.sha256, createHash('sha256').update(bytes).digest('hex'));
	assert.equal(JSON.parse(json).manifest.entry.sha256, upload.manifest.entry.sha256);
	assert.ok(!html.includes('<!--author-copy-->'));
});

test('ui_view author copy is inert data with < escaped', () => {
	const text = '</script><img src=x onerror=alert(1)> $& end';
	const html = renderUiViewHtml({ ...base, text, title: '<b>t</b>' });
	const copy = html.match(/<script type="application\/json" id="author-copy">(.+)<\/script>/)?.[1];
	assert.ok(copy);
	assert.equal(copy.includes('<'), false);
	assert.deepEqual(JSON.parse(copy), {
		title: '<b>t</b>',
		text,
		presentation: 'table',
		read: 'records'
	});
	assert.equal(html.includes(text), false);
});

test('mirrored rules reject bad v3 manifests', async () => {
	const good = (await buildUiViewPackage(all(base))).upload.manifest;
	const check = (patch: Partial<PluginManifestV3>) => validateManifestV3({ ...good, ...patch });
	assert.equal(check({ key: 'library' })?.reason, 'manifest_field_invalid');
	assert.equal(check({ bridge_version: 1 as 2 })?.reason, 'manifest_protocol_unsupported');
	assert.equal(
		check({
			capabilities: [{ name: 'amount.preview', permission: 'p', input_schema: 1, output_schema: 1 }]
		})?.reason,
		'profile_mismatch'
	);
	const [view, ...rest] = good.contributions;
	const nine = Array.from({ length: 9 }, (_, i) => ({
		...rest[0],
		id: `c${i}`,
		form_key: `f${i}`
	}));
	assert.equal(check({ contributions: nine })?.reason, 'contributions_invalid');
	const views = [1, 2, 3].map((i) => ({ ...view, id: `v${i}`, route: `r${i}` }));
	assert.deepEqual(check({ contributions: views }), { reason: 'contributions_invalid', index: 2 });
	assert.equal(
		check({ contributions: [{ ...view, kind: 'panel' }] })?.reason,
		'contributions_invalid'
	);
	assert.equal(
		check({ contributions: [{ ...view, route: null }] })?.reason,
		'contributions_invalid'
	);
	assert.equal(
		check({ contributions: [{ ...rest[0], form_key: null }] })?.reason,
		'contributions_invalid'
	);
	assert.equal(check({ contributions: [rest[0], { ...rest[0], id: 'other' }] })?.index, 1);
	const read = good.reads[0];
	assert.equal(
		check({ reads: Array.from({ length: 5 }, (_, i) => ({ ...read, id: `r${i}` })) })?.reason,
		'reads_invalid'
	);
	assert.equal(check({ reads: [{ ...read, fields: [] }] })?.reason, 'reads_invalid');
});

test('ui_view reads stored drafts defensively and the mock transport serves sample data', async () => {
	assert.deepEqual(readUiViewDraft(null, origin), base);
	assert.equal(readUiViewDraft('{"name":true,"title":"Mine"}', origin).name, base.name);
	assert.equal(readUiViewDraft('{"title":"Mine"}', origin).title, 'Mine');
	assert.deepEqual(readUiViewDraft('not json', origin), base);
	const read = { id: 'records', form_key: 'order_line', fields: ['quantity', 'note'] };
	const project = createMockTransport(
		read,
		[{ key: 'quantity', label: 'Qty', type: 'number' }],
		false
	);
	const described = await project('data.describe', { read: 'records' }, 'r');
	assert.ok(described.ok);
	const query = await project('data.query', { read: 'records' }, 'r');
	assert.ok(query.ok);
	const items = (query.result as { items: { values: Record<string, unknown> }[] }).items;
	assert.equal(items.length, 3);
	assert.deepEqual(items[0].values.quantity, { type: 'decimal', decimal: '10' });
	const personal = createMockTransport(read, null, true);
	const rows = (await personal('data.query', { read: 'records' }, 'r')) as {
		result: { items: { values: Record<string, unknown> }[] };
	};
	assert.deepEqual(rows.result.items[1].values, { quantity: '示例', note: '示例' });
	assert.deepEqual(await personal('data.query', { read: 'other' }, 'r'), {
		ok: false,
		code: 'denied'
	});
	assert.deepEqual(await personal('amount.preview', {}, 'r'), {
		ok: false,
		code: 'method_unavailable'
	});
});

test('order_amount keeps v2 output and the template review matches the registry contract', async () => {
	const tpl = creatorTemplates.order_amount;
	assert.equal(tpl.output, 'openpr.plugin.v2');
	const draft = tpl.defaults({ slot: 'form.record.actions', formKey: 'invoice_line' });
	assert.equal(draft.formKey, 'invoice_line');
	assert.equal((await tpl.build(draft)).json, (await buildCreatorPackage(draft)).json);
	assert.equal(
		tpl.read(null, { slot: 'form.record.detail', formKey: null }).formKey,
		defaultDraft.formKey
	);
	assert.deepEqual(tpl.review(draft).capabilities, ['amount.preview', 'amount.apply']);
	assert.deepEqual(
		uiViewContributions(base).map((item) => item.id),
		['project_view']
	);
});

test('upload accepts v2 and v3 and rejects a v3 manifest carrying ui', async () => {
	const v3 = (await buildUiViewPackage(base)).upload;
	assert.equal((await readPluginUpload(file(v3))).manifest.key, v3.manifest.key);
	const v2 = await buildCreatorPackage(defaultDraft);
	assert.equal((await readPluginUpload(file(v2.upload))).manifest.key, defaultDraft.key);
	await assert.rejects(
		readPluginUpload(file({ ...v3, manifest: { ...v3.manifest, ui: { form_key: 'x' } } }))
	);
	await assert.rejects(readPluginUpload(file({ ...v3, wasm_base64: 'AAAA' })));
	await assert.rejects(
		readPluginUpload(file({ ...v3, manifest: { ...v3.manifest, profile: 'x' } }))
	);
	await assert.rejects(
		readPluginUpload(
			file({ ...v3, manifest: { ...v3.manifest, contributions: [{ id: 'a', slot: 'nope' }] } })
		)
	);
	await assert.rejects(readPluginUpload(file({ ...v2.upload, wasm_base64: undefined })));
});

test('layout-v3 merges stored rows with defaults and never submits undeclared rows', async () => {
	const manifest = (await buildUiViewPackage(all(base))).upload.manifest;
	const draft = layoutDraftV3(manifest, [
		{ id: 'record_detail', enabled: false, position: 10 },
		{ id: 'removed_in_this_version', enabled: false, position: 1 }
	]);
	assert.deepEqual(
		draft.map((row) => row.id),
		['list_toolbar', 'project_view', 'record_actions', 'record_detail']
	);
	assert.deepEqual(
		draft.find((row) => row.id === 'record_detail'),
		{
			id: 'record_detail',
			enabled: false,
			position: 10
		}
	);
	assert.deepEqual(
		draft.find((row) => row.id === 'project_view'),
		{
			id: 'project_view',
			enabled: true,
			position: 50
		}
	);
	const item = {
		id: 'p',
		manifest,
		placements: [{ id: 'list_toolbar', enabled: false, position: 3 }]
	} as unknown as InstalledPluginPackage;
	assert.equal(installedLayoutDraftV3(item)?.find((row) => row.id === 'list_toolbar')?.position, 3);
	assert.equal(
		installedLayoutDraftV3({
			...item,
			manifest: { key: 'a' }
		} as unknown as InstalledPluginPackage),
		undefined
	);
});

test('layout-v3 grant helpers keep ui.display, reads and capabilities consistent', async () => {
	const manifest = (await buildUiViewPackage(base)).upload.manifest;
	let grants = toggleGrantV3(manifest, [], 'read:records', true);
	assert.deepEqual(grants, ['read:records', 'ui.display']);
	assert.ok(grantsValidV3(manifest, grants));
	assert.deepEqual(toggleGrantV3(manifest, grants, 'ui.display', false), []);
	grants = toggleGrantV3(manifest, grants, 'amount.apply', true);
	assert.deepEqual(grants, ['read:records', 'ui.display']);
	assert.equal(grantsValidV3(manifest, ['read:records']), false);
	assert.equal(grantsValidV3(manifest, ['ui.display', 'read:nope']), false);
	const wasm = {
		...manifest,
		profile: 'wasm',
		capabilities: [{ name: 'amount.preview' }, { name: 'amount.apply' }]
	} as unknown as PluginManifestV3;
	const both = toggleGrantV3(wasm, [], 'amount.apply', true);
	assert.deepEqual(both, ['amount.apply', 'amount.preview', 'ui.display']);
	assert.deepEqual(toggleGrantV3(wasm, both, 'amount.preview', false), ['ui.display']);
	assert.equal(grantsValidV3(wasm, ['amount.apply', 'ui.display']), false);
});
