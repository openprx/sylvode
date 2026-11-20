#!/usr/bin/env node
// Runs every `tests/forms-page/*.test.ts` suite with Bun, one process per file (the pattern of
// scripts/run-ui-tests.mjs). Zero files is a pass until the unit tests of the forms page split
// exist.
//
//   node scripts/run-forms-page-tests.mjs
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';

const root = new URL('../', import.meta.url).pathname;
const dir = process.argv[2] ? resolve(process.argv[2]) : join(root, 'tests/forms-page');

function find(folder, found = []) {
	if (!existsSync(folder)) return found;
	for (const name of readdirSync(folder)) {
		const path = join(folder, name);
		if (statSync(path).isDirectory()) find(path, found);
		else if (name.endsWith('.test.ts')) found.push(path);
	}
	return found;
}

let failed = 0;
const files = find(dir).sort();
for (const file of files) {
	console.log(`\n# ${relative(root, file)}`);
	const result = spawnSync('bun', [file], { cwd: root, stdio: 'inherit' });
	if (result.status !== 0) failed += 1;
}
console.log(`\n${files.length} suites, ${failed} failed`);
process.exit(failed > 0 || files.length === 0 ? 1 : 0);
