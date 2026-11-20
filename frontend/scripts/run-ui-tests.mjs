#!/usr/bin/env node
// Runs every `src/lib/ui/**/*.test.ts` suite with Bun, one process per file, so a component adds
// its test next to itself and nothing here needs editing.
//
//   node scripts/run-ui-tests.mjs
import { spawnSync } from 'node:child_process';
import { readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = new URL('../', import.meta.url).pathname;

function find(dir, found = []) {
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) find(path, found);
		else if (name.endsWith('.test.ts')) found.push(path);
	}
	return found;
}

let failed = 0;
const files = find(join(root, 'src/lib/ui')).sort();
for (const file of files) {
	console.log(`\n# ${relative(root, file)}`);
	const result = spawnSync('bun', [file], { cwd: root, stdio: 'inherit' });
	if (result.status !== 0) failed += 1;
}
console.log(`\n${files.length} suites, ${failed} failed`);
process.exit(failed > 0 || files.length === 0 ? 1 : 0);
