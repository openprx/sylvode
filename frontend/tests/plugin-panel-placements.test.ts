import assert from 'node:assert/strict';
import { test } from 'node:test';
import type { InstalledPluginPackage, PluginPanel } from '../src/lib/api/plugin-packages';
import {
	panelEntries,
	placementDraft,
	sameEntry,
	validPlacementPositions
} from '../src/lib/plugins/placements';

const legacy: PluginPanel = {
	package_id: 'a',
	key: 'example',
	version: '1.0.0',
	name: 'Order amount',
	generation: 1,
	grants: ['amount.preview'],
	html: '',
	web_sha256: '',
	package_sha256: ''
};

const optedIn: PluginPanel = {
	...legacy,
	package_id: 'b',
	version: '3.0.0',
	generation: 2,
	contributions: [
		{ id: 'detail_panel', slot: 'form.record.detail', label: 'Order amount panel', position: 70 },
		{
			id: 'amount_action',
			slot: 'form.record.actions',
			label: 'Calculate order amount',
			position: 3
		}
	]
};

test('legacy discovery only renders the detail entry and an empty explicit layout renders no launcher', () => {
	assert.deepEqual(panelEntries([legacy], 'form.record.actions'), []);
	assert.deepEqual(
		panelEntries([legacy], 'form.record.detail').map((entry) => entry.label),
		['Order amount']
	);
	assert.deepEqual(panelEntries([{ ...optedIn, contributions: [] }], 'form.record.detail'), []);
});

test('each enabled entry stays in its declared slot and a tied priority sorts by key', () => {
	const alternate = { ...optedIn, key: 'another', package_id: 'c' };
	const actionEntries = panelEntries([optedIn, alternate], 'form.record.actions');
	assert.deepEqual(
		actionEntries.map((entry) => entry.panel.package_id),
		['c', 'b']
	);
	assert.equal(actionEntries[1].slot, 'form.record.actions');
	const [detailEntry] = panelEntries([optedIn], 'form.record.detail');
	assert.equal(detailEntry.slot, 'form.record.detail');
	assert.equal(sameEntry(detailEntry, actionEntries[1]), false);
	assert.equal(sameEntry(detailEntry, detailEntry), true);
});

test('the layout editor preserves configured disabled entries and zero priority and rejects invalid priorities', () => {
	const item: InstalledPluginPackage = {
		id: 'b',
		manifest: {
			key: 'example',
			name: 'Order amount',
			version: '3.0.0',
			publisher: 'Example',
			host_protocol: { min: 2, max: 2 },
			wasm_abi: 'openpr.plugin.v1',
			ui: {
				bridge_version: 1,
				slot: 'form.record.detail',
				form_key: 'order_line',
				contributions: optedIn.contributions
			},
			capabilities: []
		},
		installed_by: 'actor',
		package_sha256: 'digest',
		placements: [{ id: 'amount_action', enabled: false, position: 0 }]
	};
	const draft = placementDraft(item);
	assert.deepEqual(draft, [
		{ id: 'detail_panel', enabled: true, position: 50 },
		{ id: 'amount_action', enabled: false, position: 0 }
	]);
	assert.equal(validPlacementPositions(draft), true);
	assert.equal(
		validPlacementPositions([{ id: 'amount_action', enabled: true, position: NaN }]),
		false
	);
	assert.equal(
		validPlacementPositions([{ id: 'detail_panel', enabled: true, position: 100 }]),
		false
	);
});
