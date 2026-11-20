import assert from 'node:assert/strict';
import { describe, test } from 'node:test';
import { focusReviewStart } from '../src/lib/plugins/creator/review-focus';

describe('creator review focus', () => {
	test('focuses the heading without scrolling', () => {
		const calls: Array<FocusOptions | undefined> = [];
		focusReviewStart({ focus: (options) => void calls.push(options) });
		assert.deepEqual(calls, [{ preventScroll: true }]);
	});

	test('does nothing before the heading is mounted', () => {
		assert.doesNotThrow(() => focusReviewStart(undefined));
		assert.doesNotThrow(() => focusReviewStart(null));
	});
});
