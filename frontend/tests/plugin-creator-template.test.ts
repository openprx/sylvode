import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { test } from 'node:test';
import {
	buildCreatorPackage,
	creatorPlacement,
	creatorSlot,
	defaultDraft,
	invalidDraftFields,
	readDraft
} from '../src/lib/plugins/creator/template';
import { validateManifestV3 } from '../src/lib/plugins/creator/registry';

test('creator builds deterministic protocol 3 wasm bytes with both locations and the template’s fixed guest', async () => {
	const draft = {
		...defaultDraft,
		key: 'billing_labels',
		formKey: 'invoice_line',
		detailTitle: 'Line item breakdown',
		actionLabel: 'Calculate invoice'
	};
	const created = await buildCreatorPackage(draft);
	assert.equal(created.json, (await buildCreatorPackage(draft)).json);
	assert.equal(created.fixtureCents, 2500);
	const manifest = created.upload.manifest;
	assert.equal(manifest.schema_version, 'openpr.plugin.v3');
	assert.equal(manifest.profile, 'wasm');
	assert.equal(manifest.bridge_version, 2);
	assert.deepEqual(manifest.host_protocol, { min: 3, max: 3 });
	assert.equal(manifest.key, draft.key);
	assert.equal(manifest.wasm?.form_key, draft.formKey);
	assert.deepEqual(manifest.reads, []);
	assert.deepEqual(
		manifest.contributions.map((value) => [
			value.id,
			value.kind,
			value.slot,
			value.view,
			value.form_key
		]),
		[
			['detail_panel', 'panel', 'form.record.detail', 'detail', 'invoice_line'],
			['amount_action', 'action', 'form.record.actions', 'actions', 'invoice_line']
		]
	);
	assert.deepEqual(
		manifest.contributions.map((value) => value.label),
		[defaultDraft.detailLabel, draft.actionLabel]
	);
	assert.deepEqual(
		manifest.capabilities.map((item) => [item.name, item.permission]),
		[
			['amount.preview', 'record.read'],
			['amount.apply', 'record.update']
		]
	);
	assert.equal(validateManifestV3(manifest), null);
	const bytes = Buffer.from(created.upload.web_base64, 'base64');
	const wire = JSON.parse(created.json);
	assert.equal(bytes.toString('utf8'), created.html);
	assert.equal(wire.manifest.entry.size, bytes.length);
	assert.equal(wire.manifest.entry.sha256, createHash('sha256').update(bytes).digest('hex'));
	assert.equal('ui' in wire.manifest, false);
	const wasm = Buffer.from(wire.wasm_base64, 'base64');
	assert.equal(wire.manifest.wasm.sha256, createHash('sha256').update(wasm).digest('hex'));
	assert.equal(wire.manifest.wasm.size, wasm.length);
});

test('the panel speaks bridge protocol 2 only and shows the 25.00 preview contract', async () => {
	const { html } = await buildCreatorPackage(defaultDraft);
	assert.ok(html.includes("protocol: 2,\n\t\t\t\ttype: 'request'"));
	assert.equal(html.includes('protocol: 1'), false);
	assert.equal(html.includes('event.data.placement'), false);
	assert.ok(
		html.includes("method: 'amount.preview'") || html.includes("request('amount.preview')")
	);
	assert.ok(html.includes("type: 'resize'"));
});

test('author copy remains data and preserves special characters literally', async () => {
	const actionText = '$& in <em>quoted text</em>';
	const created = await buildCreatorPackage({ ...defaultDraft, actionText });
	const text = created.html.match(
		/<script type="application\/json" id="author-copy">(.+)<\/script>/
	)?.[1];
	assert.ok(text);
	assert.equal(JSON.parse(text).actionText, actionText);
	assert.equal(created.html.includes(actionText), false);
	assert.ok(created.html.includes('copy.actionText'));
});

test('creator restores only typed draft fields and validates Form key, package key and version', () => {
	assert.equal(readDraft(null, 'invoice_line').formKey, 'invoice_line');
	assert.equal(readDraft('{"name":true}', 'invoice_line').name, defaultDraft.name);
	assert.equal(readDraft('{"name":"Local draft"}').name, 'Local draft');
	assert.deepEqual(
		invalidDraftFields({ ...defaultDraft, key: 'bad-key', formKey: '/form', version: 'v1' }),
		['key', 'version', 'formKey']
	);
	assert.deepEqual(creatorPlacement(creatorSlot('form.record.actions')), {
		id: 'amount_action',
		slot: 'form.record.actions'
	});
	assert.deepEqual(creatorPlacement(creatorSlot('unknown')), {
		id: 'detail_panel',
		slot: 'form.record.detail'
	});
});
