import { describe, test } from 'node:test';
import assert from 'node:assert/strict';
import {
	cloneFormsSnapshot,
	formsDraftDirty,
	formsFingerprint,
	FormsRefreshLatch,
	mayRefreshForms,
	recordWriteVersion,
	restoredFormsSnapshot,
	type FormsSessionSnapshot
} from '../src/lib/workbench/forms-session';
const snapshot = (): FormsSessionSnapshot => ({
	formId: 'form-a',
	state: { selectedRecordId: 'record-a', recordDraft: { note: 'unsaved' }, filter: 'needle' },
	clean: { record: formsFingerprint({ note: 'original' }) },
	formSchemaVersion: 3,
	recordVersion: { recordId: 'record-a', updatedAt: '2026-11-15T07:00:00.123456Z', schemaVersion: 3 },
	viewUpdatedAt: '2026-11-15T07:00:00.654321Z'
});
describe('retained Forms session contract', () => {
	test('restores only the same reauthorized resource, preserving original microsecond token', () => {
		const saved = snapshot();
		assert.equal(restoredFormsSnapshot(saved, 'form-a', false), null);
		assert.equal(restoredFormsSnapshot(saved, 'form-b', true), null);
		const restored = restoredFormsSnapshot(saved, 'form-a', true)!;
		assert.equal(restored.recordVersion?.updatedAt, '2026-11-15T07:00:00.123456Z');
		assert.deepEqual(restored.state.recordDraft, { note: 'unsaved' });
		restored.state.recordDraft = { note: 'changed' };
		assert.deepEqual(saved.state.recordDraft, { note: 'unsaved' });
	});
	test('fresh server version never silently rebases a record draft', () => {
		const base = snapshot().recordVersion;
		assert.deepEqual(recordWriteVersion('record-a', base, { id: 'record-a', updated_at: 'newer' }, 4), {
			expected_updated_at: '2026-11-15T07:00:00.123456Z', expected_schema_version: 3
		});
		assert.equal(recordWriteVersion('missing', null, null, 4), null);
	});
	test('dirty is based on editable data rather than new server metadata', () => {
		const clean = { record: formsFingerprint({ note: 'original' }) };
		assert.equal(formsDraftDirty({ record: { note: 'original' } }, clean), false);
		assert.equal(formsDraftDirty({ record: { note: 'unsaved' } }, clean), true);
		assert.equal(cloneFormsSnapshot(snapshot()).state.filter, 'needle');
	});
	test('background/hidden/offline/saving pages do not poll', () => {
		const active = { active: true, visible: true, online: true, saving: false, loading: false };
		assert.equal(mayRefreshForms(active), true);
		for (const state of [{ active: false }, { visible: false }, { online: false }, { saving: true }, { loading: true }])
			assert.equal(mayRefreshForms({ ...active, ...state }), false);
	});
	test('refresh never overlaps; a failed read releases the latch without changing snapshots', async () => {
		const latch = new FormsRefreshLatch();
		let finish!: () => void;
		const first = latch.run(() => new Promise<void>((resolve) => { finish = resolve; }));
		assert.equal(await latch.run(async () => {}), false);
		finish();
		assert.equal(await first, true);
		await assert.rejects(latch.run(async () => { throw new Error('offline'); }), /offline/);
		assert.equal(await latch.run(async () => {}), true);
		latch.dispose();
		assert.equal(await latch.run(async () => {}), false);
	});
});
