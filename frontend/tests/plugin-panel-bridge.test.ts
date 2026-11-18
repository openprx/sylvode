import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import type { PluginInvocation, PluginPanel } from '../src/lib/api/plugin-packages';
import { pluginRequest, samePanel, validResult, verifyPanelHtml } from '../src/lib/plugins/bridge';

const requestId = 'dea0894d-cc5d-4c1f-85d1-9694dcef60bb';
const panel: PluginPanel = {
	package_id: 'package-a',
	key: 'order-amount',
	version: '1.0.0',
	name: 'Order amount',
	generation: 2,
	grants: ['amount.preview', 'amount.apply'],
	html: 'abc',
	web_sha256: 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
	package_sha256: 'package-digest'
};

describe('record plugin bridge', () => {
	test('accepts only its protocol, request identifiers, and empty input on declared host methods', () => {
		const valid = {
			protocol: 1,
			type: 'request',
			request_id: requestId,
			method: 'amount.preview',
			input: {}
		};
		assert.deepEqual(pluginRequest(valid), { method: 'amount.preview', requestId });
		assert.equal(pluginRequest({ ...valid, request_id: 'foreign' }), undefined);
		assert.equal(pluginRequest({ ...valid, protocol: 2 }), undefined);
		assert.equal(pluginRequest({ ...valid, method: 'fetch' })?.method, undefined);
		assert.equal(pluginRequest({ ...valid, input: { record_id: 'foreign' } })?.method, undefined);
	});

	test('verifies exact HTML bytes and detects version, artifact and grant changes', async () => {
		assert.equal(await verifyPanelHtml(panel), true);
		assert.equal(await verifyPanelHtml({ ...panel, html: panel.html + '\n' }), false);
		assert.equal(samePanel(panel, { ...panel, grants: [...panel.grants].reverse() }), true);
		assert.equal(samePanel(panel, { ...panel, grants: ['amount.preview'] }), false);
		assert.equal(samePanel(panel, { ...panel, generation: 3 }), false);
		assert.equal(samePanel(panel, { ...panel, package_sha256: 'replacement' }), false);
	});

	test('accepts only the matching response and the bounded integer amount schema', () => {
		const response: PluginInvocation = {
			package_id: panel.package_id,
			version: panel.version,
			generation: panel.generation,
			amount_cents: 1325,
			request_id: requestId
		};
		assert.equal(validResult(response, panel, requestId), true);
		assert.equal(validResult({ ...response, generation: 3 }, panel, requestId), false);
		assert.equal(validResult({ ...response, amount_cents: -1 }, panel, requestId), false);
		assert.equal(validResult({ ...response, amount_cents: 1.25 }, panel, requestId), false);
		assert.equal(validResult({ ...response, amount_cents: 1_000_000_000_001 }, panel, requestId), false);
	});
});
