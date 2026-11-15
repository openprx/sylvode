import { test } from 'node:test';
import assert from 'node:assert/strict';
import { commentActions, FlowMutationIntents } from '../src/lib/flow/social-actions';
import type { FlowComment } from '../src/lib/api/flow';
const comment = { author_id: 'author' } as FlowComment;
test('author, editor and manager comment affordances follow shared service rules', () => {
	assert.deepEqual(
		commentActions(comment, 'author', { comment: true, edit: false, manage: false }),
		{ edit: true, delete: true, resolve: true }
	);
	assert.deepEqual(
		commentActions(comment, 'reader', { comment: true, edit: false, manage: false }),
		{ edit: false, delete: false, resolve: false }
	);
	assert.deepEqual(
		commentActions(comment, 'editor', { comment: true, edit: true, manage: false }),
		{ edit: false, delete: false, resolve: true }
	);
	assert.deepEqual(
		commentActions(comment, 'manager', { comment: true, edit: true, manage: true }),
		{ edit: false, delete: true, resolve: true }
	);
	assert.deepEqual(
		commentActions(comment, 'author', { comment: false, edit: false, manage: false }),
		{ edit: false, delete: false, resolve: false }
	);
});
test('failed-operation retries reuse one key, acknowledged/new payload and identity use new intent', () => {
	let next = 0;
	const intents = new FlowMutationIntents(() => `intent-${++next}`);
	const operation = 'user-a:object:comment';
	assert.equal(intents.key(operation, { body: 'draft' }), 'intent-1');
	assert.equal(intents.key(operation, { body: 'draft' }), 'intent-1');
	assert.equal(intents.key(operation, { body: 'changed draft' }), 'intent-2');
	assert.equal(intents.key('user-b:object:comment', { body: 'draft' }), 'intent-3');
	intents.complete(operation);
	assert.equal(intents.key(operation, { body: 'changed draft' }), 'intent-4');
});
