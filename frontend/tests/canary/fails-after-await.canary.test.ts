// Canary for scripts/run-tests.mjs. It fails on purpose, after an `await`, the shape in which
// `bun test` once reported a failing node:test assertion as a pass. The runner executes it with
// `bun <file>` in expect-failure mode and the job fails when the runner reports it as passed.
// The name does not end in `.test.ts`, so the normal glob never picks it up.
import { test } from 'node:test';
import assert from 'node:assert/strict';

test('canary: assertion after an await must fail', async () => {
	await new Promise((resolve) => setTimeout(resolve, 5));
	assert.equal(1, 2, 'deliberate canary failure');
});
