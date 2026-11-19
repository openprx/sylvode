import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import {
	closePreview,
	initialPreviewFrame,
	retirePreview,
	retryPreview
} from '../src/lib/plugins/creator/preview-frame';

describe('creator preview frame', () => {
	test('a retired preview mounts a fresh frame', () => {
		const state = initialPreviewFrame();
		assert.equal(retirePreview(state, 0), true);
		assert.equal(state.attempt, 1);
		assert.equal(state.closed, null);
	});

	test('a stale report is ignored', () => {
		const state = initialPreviewFrame();
		retirePreview(state, 0);
		assert.equal(retirePreview(state, 0), false);
		assert.equal(state.attempt, 1);
	});

	test('a preview closed with a notice is not reopened by a late report', () => {
		const state = initialPreviewFrame();
		closePreview(state, 'plugins.host.loadFailed');
		assert.equal(retirePreview(state, 0), false);
		assert.equal(state.closed, 'plugins.host.loadFailed');
		assert.equal(state.attempt, 0);
	});

	test('retry mounts a new frame and clears the notice; the old host cannot reopen', () => {
		const state = initialPreviewFrame();
		closePreview(state, '');
		retryPreview(state);
		assert.equal(state.closed, null);
		assert.equal(state.attempt, 1);
		assert.equal(retirePreview(state, 0), false);
		assert.equal(retirePreview(state, 1), true);
		assert.equal(state.attempt, 2);
	});
});
