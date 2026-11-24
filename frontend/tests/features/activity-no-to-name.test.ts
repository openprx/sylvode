/**
 * Nothing on the server writes `detail.to_name` into an activity entry (assignments, sprints and
 * milestones carry ids only), so no page reads it.
 *
 * Run standalone: `bun test tests/features/activity-no-to-name.test.ts`
 */
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const SRC = new URL('../../src', import.meta.url).pathname;

function files(dir: string, found: string[] = []): string[] {
	for (const entry of readdirSync(dir)) {
		const path = join(dir, entry);
		if (statSync(path).isDirectory()) files(path, found);
		else if (/\.(ts|svelte)$/.test(entry)) found.push(path);
	}
	return found;
}

test('no source file reads an activity detail to_name', () => {
	const hits = files(SRC)
		.filter((path) => /\bto_name\b/.test(readFileSync(path, 'utf8')))
		.map((path) => relative(SRC, path).split(sep).join('/'));
	assert.deepEqual(hits, []);
});
