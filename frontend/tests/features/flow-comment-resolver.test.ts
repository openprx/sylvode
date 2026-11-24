/**
 * A Flow comment's `resolved_by` is a person object, shown directly: the panel looks nothing up
 * and never reads it as an id.
 *
 * Run standalone: `bun test tests/features/flow-comment-resolver.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = (path: string) => readFileSync(new URL(path, import.meta.url), 'utf8');

test('the comment type carries the resolver as a person and the panel shows it directly', () => {
	const api = read('../../src/lib/api/flow.ts');
	assert.match(api, /resolved_by\?: Person \| null;/);
	const panel = read('../../src/lib/components/flow/FlowCommentsPanel.svelte');
	assert.match(panel, /personLabel\(comment\.resolved_by, \$t\)/);
	assert.ok(!panel.includes('knownPeople'));
	assert.ok(!panel.includes('personLabelById'));
});
