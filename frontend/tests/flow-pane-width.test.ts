import assert from 'node:assert/strict';
import { test } from 'node:test';
import { paneBounds, clampPaneWidth } from '../src/lib/flow/pane-width';
test('desktop panes retain a useful editor area and clamp saved preferences after resize', () => {
	const wide = paneBounds('right', 1400);
	assert.equal(wide.max, 520);
	assert.equal(wide.initial, 352);
	const narrow = paneBounds('right', 600);
	assert.equal(narrow.max, 228);
	assert.equal(clampPaneWidth(500, narrow.min, narrow.max), 228);
	assert.ok(600 - narrow.max >= 360);
	const compact = paneBounds('right', 480);
	assert.equal(compact.max, 120);
	assert.equal(compact.min, 120);
});
test('pane defaults, keyboard boundaries and invalid saved numbers are bounded', () => {
	const left = paneBounds('left', 1400);
	assert.deepEqual(left, { min: 200, max: 400, initial: 256 });
	assert.equal(clampPaneWidth(-99, left.min, left.max), 200);
	assert.equal(clampPaneWidth(Infinity, left.min, left.max), 200);
	assert.equal(clampPaneWidth(999, left.min, left.max), 400);
});
