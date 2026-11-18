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

test('creator builds deterministic installable bytes with both entries and the template’s fixed guest', async () => {
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
	assert.equal(created.upload.manifest.key, draft.key);
	assert.equal(created.upload.manifest.ui.form_key, draft.formKey);
	assert.deepEqual(
		created.upload.manifest.ui.contributions?.map((value) => value.label),
		[defaultDraft.detailLabel, draft.actionLabel]
	);
	const bytes = Buffer.from(created.upload.web_base64, 'base64');
	const wire = JSON.parse(created.json);
	assert.equal(bytes.toString('utf8'), created.html);
	assert.equal(wire.manifest.ui.entry.size, bytes.length);
	assert.equal(wire.manifest.ui.entry.sha256, createHash('sha256').update(bytes).digest('hex'));
	assert.equal(
		wire.manifest.wasm.sha256,
		createHash('sha256').update(Buffer.from(wire.wasm_base64, 'base64')).digest('hex')
	);
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
	assert.ok(created.html.includes('textContent = author.actionText'));
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
