import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { PluginContributionItem } from '../src/lib/api/plugin-packages';
import type { DiscoveryState, EntryOutcome } from '../src/lib/plugins/contributions';
import {
	closeSlot,
	createApplyConfirmation,
	initialSlot,
	openSlot,
	recordPreview,
	reopenDecision,
	retireSlot,
	type SlotDeps,
	type SlotState
} from '../src/lib/plugins/frame-slot';
import type { RecordEntry } from '../src/lib/plugins/record-entries';

function item(overrides: Partial<PluginContributionItem> = {}): PluginContributionItem {
	return {
		package_id: 'pkg',
		key: 'order_amount',
		version: '1.0.0',
		name: 'Order amount',
		profile: 'ui',
		generation: 1,
		package_sha256: 'p',
		web_sha256: 'w',
		contribution: {
			id: 'amount',
			kind: 'panel',
			slot: 'form.record.detail',
			label: 'Amount',
			view: 'amount',
			route: null,
			form_key: null
		},
		project_position: 50,
		preference: { hidden: false, collapsed: false, position: null },
		effective_position: 50,
		grants: ['ui.display', 'amount.preview', 'amount.apply'],
		...overrides
	};
}

const ready = (items: PluginContributionItem[]): DiscoveryState => ({ status: 'ready', items });
const contributionEntry = (value = item()): RecordEntry => ({
	item: value,
	id: value.contribution.id,
	label: value.contribution.label,
	slot: 'form.record.detail',
	position: 50
});

/** Drives the machine the way RecordPluginPanels and FormPluginToolbar do. */
function area(
	entry: (target: PluginContributionItem) => EntryOutcome = (target) => ({
		ok: true,
		item: target,
		html: '<p>x</p>'
	})
) {
	const state: SlotState<RecordEntry> = initialSlot();
	const calls = { entries: 0, refreshes: 0, hidden: 0, shown: 0 };
	const confirmation = createApplyConfirmation({
		show: () => calls.shown++,
		hide: () => calls.hidden++
	});
	const deps: SlotDeps<RecordEntry> = {
		itemOf: (e) => e.item,
		loadEntry: async (target) => {
			calls.entries++;
			return entry(target);
		},
		projectName: async () => 'Demo',
		refresh: () => calls.refreshes++,
		dismissConfirmation: () => confirmation.dismiss()
	};
	let discovery: DiscoveryState = ready([item()]);
	return {
		state,
		calls,
		confirmation,
		deps,
		set: (next: DiscoveryState) => (discovery = next),
		open: (e: RecordEntry) => openSlot(state, e, deps),
		retire: (token: number) => retireSlot(state, token, discovery, deps)
	};
}

/** The area shows something: closed (with or without a notice), loading, or a frame. */
function assertConsistent(state: SlotState<RecordEntry>) {
	if (state.frame) assert.ok(state.active, 'a frame without an open entry');
	if (!state.active) assert.equal(state.frame, null);
}

describe('frame slot: retire and reopen', () => {
	test('a retired frame reopens with a fresh entry, a new token, and the same entry open', async () => {
		const a = area();
		const first = await a.open(contributionEntry());
		assert.ok(first);
		assert.equal(a.calls.entries, 1);
		const pending = a.retire(first);
		// Never an empty area: the entry stays open and loading.
		assert.equal(a.state.frame, null);
		assert.ok(a.state.active);
		assertConsistent(a.state);
		assert.equal(await pending, 'reopened');
		assert.equal(a.calls.entries, 2, 'a fresh entry request');
		assert.notEqual(a.state.frame!.token, first, 'never the retired host');
		assert.equal(a.state.notice, '');
		assertConsistent(a.state);
	});

	test('stale token is ignored: after a reopen, after another open, and after a close', async () => {
		const a = area();
		const first = await a.open(contributionEntry());
		assert.equal(await a.retire(first), 'reopened');
		const second = a.state.frame!.token;
		assert.equal(await a.retire(first), 'ignored');
		assert.equal(a.state.frame!.token, second);
		assert.equal(a.calls.entries, 2);
		closeSlot(a.state, a.deps);
		assert.equal(await a.retire(second), 'ignored');
		assert.equal(a.state.active, null);
		assert.equal(a.calls.entries, 2);
	});

	test('a frame the user closed is not reopened and its notice stays empty', async () => {
		const a = area();
		const token = await a.open(contributionEntry());
		closeSlot(a.state, a.deps);
		assert.equal(await a.retire(token), 'ignored');
		assert.equal(a.state.active, null);
		assert.equal(a.state.frame, null);
		assert.equal(a.state.notice, '');
		assert.equal(a.calls.entries, 1);
	});

	test('a retire while the entry is still loading changes nothing', async () => {
		const a = area();
		const opening = a.open(contributionEntry());
		assert.equal(await a.retire(1), 'ignored');
		assert.ok(await opening);
		assert.equal(a.calls.entries, 1);
	});

	test('contribution gone: no reopen and the access notice', async () => {
		const a = area();
		const token = await a.open(contributionEntry());
		a.set(ready([]));
		assert.equal(await a.retire(token), 'closed');
		assert.equal(a.calls.entries, 1, 'no entry request for a revoked contribution');
		assert.equal(a.state.active, null);
		assert.equal(a.state.frame, null);
		assert.equal(a.state.notice, 'plugins.accessChanged');
	});

	test('extensions off or denied: no reopen', async () => {
		for (const status of ['off', 'denied'] as const) {
			const a = area();
			const token = await a.open(contributionEntry());
			a.set({ status, items: [] });
			assert.equal(await a.retire(token), 'closed');
			assert.equal(a.state.notice, 'plugins.accessChanged');
			assert.equal(a.calls.entries, 1);
		}
	});

	test('discovery error: no reopen and the check-failed notice', async () => {
		const a = area();
		const token = await a.open(contributionEntry());
		a.set({ status: 'error', items: [item()] });
		assert.equal(await a.retire(token), 'closed');
		assert.equal(a.state.notice, 'plugins.checkFailed');
		assert.equal(a.calls.entries, 1);
	});

	test('contribution changed (version, digest, grants): no reopen and the changed notice', async () => {
		const changes: Partial<PluginContributionItem>[] = [
			{ version: '1.1.0' },
			{ web_sha256: 'other' },
			{ grants: ['ui.display', 'amount.preview'] },
			{ generation: 2 }
		];
		for (const change of changes) {
			const a = area();
			const token = await a.open(contributionEntry());
			a.set(ready([item(change)]));
			assert.equal(await a.retire(token), 'closed', JSON.stringify(change));
			assert.equal(a.state.notice, 'plugins.versionChanged');
			assert.equal(a.state.frame, null);
			assert.equal(a.calls.entries, 1);
		}
	});

	test('the fresh entry answering for a different identity is not shown', async () => {
		let version = '1.0.0';
		const a = area(() => ({ ok: true, item: item({ version }), html: '<p>x</p>' }));
		const token = await a.open(contributionEntry());
		version = '2.0.0';
		assert.equal(await a.retire(token), 'closed');
		assert.equal(a.state.notice, 'plugins.versionChanged');
		assert.equal(a.state.frame, null);
		assert.equal(a.calls.refreshes, 1, 'the discovery is asked to catch up');
	});

	test('the fresh entry failing closes with a notice instead of showing nothing', async () => {
		let outcome: EntryOutcome = { ok: true, item: item(), html: '<p>x</p>' };
		const a = area(() => outcome);
		const token = await a.open(contributionEntry());
		outcome = { ok: false, reason: 'failed' };
		assert.equal(await a.retire(token), 'closed');
		assert.equal(a.state.notice, 'plugins.host.loadFailed');
		assert.equal(a.state.active, null);
		outcome = { ok: false, reason: 'unavailable' };
		const again = await a.open(contributionEntry());
		assert.equal(again, 0);
		assert.equal(a.state.notice, 'plugins.accessChanged');
	});

	test('a retire superseded by a close while the fresh entry is in flight is dropped', async () => {
		let release: (outcome: EntryOutcome) => void = () => undefined;
		let first = true;
		const state: SlotState<RecordEntry> = initialSlot();
		const deps: SlotDeps<RecordEntry> = {
			itemOf: (e) => e.item,
			loadEntry: () =>
				first
					? ((first = false), Promise.resolve({ ok: true, item: item(), html: '' }))
					: new Promise((resolve) => (release = resolve)),
			projectName: async () => '',
			refresh: () => undefined
		};
		const token = await openSlot(state, contributionEntry(), deps);
		const retiring = retireSlot(state, token, ready([item()]), deps);
		closeSlot(state, deps);
		release({ ok: true, item: item(), html: '' });
		await retiring;
		assert.equal(state.active, null);
		assert.equal(state.frame, null);
	});

	test('the user switching entry before the retire report keeps the new one', async () => {
		const other = item({
			contribution: { ...item().contribution, id: 'other', label: 'Other', view: 'other' }
		});
		const a = area();
		a.set(ready([item(), other]));
		const first = await a.open(contributionEntry());
		const second = await a.open(contributionEntry(other));
		assert.notEqual(first, second);
		assert.equal(await a.retire(first), 'ignored');
		assert.equal(a.state.frame!.token, second);
	});

	test('every open and reopen mints a unique token', async () => {
		const a = area();
		const tokens = new Set<number>();
		let token = await a.open(contributionEntry());
		for (let n = 0; n < 4; n++) {
			tokens.add(token);
			await a.retire(token);
			token = a.state.frame!.token;
		}
		closeSlot(a.state, a.deps);
		tokens.add(await a.open(contributionEntry()));
		assert.equal(tokens.size, 5);
	});
});

describe('frame slot: apply confirmation and preview', () => {
	test('a pending confirmation is dismissed on retire and the panel reopens idle', async () => {
		const a = area();
		const token = await a.open(contributionEntry());
		recordPreview(a.state, token, 1250);
		const answer = a.confirmation.request();
		assert.equal(a.calls.shown, 1);
		assert.equal(a.state.preview, 1250);
		assert.equal(await a.retire(token), 'reopened');
		assert.equal(await answer, false, 'never confirmed');
		assert.ok(a.calls.hidden >= 1, 'the dialog is hidden');
		assert.equal(a.state.preview, null, 'the old amount is not shown again');
		assert.equal(a.state.notice, '');
	});

	test('a confirmation is dismissed when the retire closes the area too', async () => {
		const a = area();
		const token = await a.open(contributionEntry());
		const answer = a.confirmation.request();
		a.set(ready([]));
		assert.equal(await a.retire(token), 'closed');
		assert.equal(await answer, false);
	});

	test('a stale retire leaves a pending confirmation alone', async () => {
		const a = area();
		const old = await a.open(contributionEntry());
		await a.retire(old);
		const answer = a.confirmation.request();
		assert.equal(await a.retire(old), 'ignored');
		a.confirmation.confirm();
		assert.equal(await answer, true);
	});

	test('a new question answers an unanswered one no', async () => {
		const a = area();
		const first = a.confirmation.request();
		const second = a.confirmation.request();
		assert.equal(await first, false);
		a.confirmation.confirm();
		assert.equal(await second, true);
	});

	test('a preview reported by a frame that is no longer shown is dropped', async () => {
		const a = area();
		const first = await a.open(contributionEntry());
		await a.retire(first);
		recordPreview(a.state, first, 999);
		assert.equal(a.state.preview, null);
		recordPreview(a.state, a.state.frame!.token, 100);
		assert.equal(a.state.preview, 100);
		closeSlot(a.state, a.deps);
		recordPreview(a.state, a.state.tokens, 5);
		assert.equal(a.state.preview, null);
	});
});

describe('reopen decision', () => {
	test('uses the latest discovery item as the target', () => {
		const latest = item({ preference: { hidden: false, collapsed: true, position: 3 } });
		const decision = reopenDecision(ready([latest]), item());
		assert.deepEqual(decision, { kind: 'reopen', target: latest });
	});

	test('discovery still loading has no verdict and reopens for the opened item', () => {
		const opened = item();
		assert.deepEqual(reopenDecision({ status: 'loading', items: [] }, opened), {
			kind: 'reopen',
			target: opened
		});
	});
});
