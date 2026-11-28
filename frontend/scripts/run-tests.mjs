#!/usr/bin/env node
// Runs every frontend test file, one process per file, and exits non-zero on any failure.
//
// Two styles exist. Files that import `node:test` run with `bun test ./<file>`; the others carry
// their own harness and run with `bun <file>` (non-zero exit on failure). `bun test` of 1.2.x can
// report a node:test assertion that fails after an `await` as a pass with exit 0, so before
// anything else the runner executes canaries that fail on purpose after an `await`, one per mode
// (tests/canary). When a canary is reported as passed, the run fails: failures cannot be trusted
// with that Bun version. CI pins a Bun version that passes the canaries (see .github/workflows).
//
//   node scripts/run-tests.mjs                 every tests/**/*.test.ts and src/**/*.test.ts file
//   node scripts/run-tests.mjs <path>...       only the given files or folders
//   node scripts/run-tests.mjs --canary        only the canaries
//
import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';

const root = new URL('../', import.meta.url).pathname;
const args = process.argv.slice(2);
const canaryOnly = args.includes('--canary');
const targets = args.filter((arg) => !arg.startsWith('--'));

function find(path, found = []) {
	if (!existsSync(path)) return found;
	if (statSync(path).isFile()) {
		found.push(path);
		return found;
	}
	for (const name of readdirSync(path).sort()) {
		if (name === 'node_modules' || name === 'fixtures' || name === 'canary') continue;
		const child = join(path, name);
		if (statSync(child).isDirectory()) find(child, found);
		else if (name.endsWith('.test.ts')) found.push(child);
	}
	return found;
}

function usesNodeTest(file) {
	return /from\s+['"]node:test['"]/.test(readFileSync(file, 'utf8'));
}

function run(file) {
	const args = usesNodeTest(file) ? ['test', `./${relative(root, file)}`] : [file];
	return spawnSync('bun', args, { cwd: root, stdio: 'inherit' }).status;
}

for (const name of ['fails-after-await.canary.test.ts', 'fails-after-await.harness-canary.ts']) {
	const file = join(root, 'tests/canary', name);
	console.log(`# canary ${name} (expected to fail)`);
	if (run(file) === 0) {
		console.error(`\nCANARY BROKEN: ${name} was reported as passed.`);
		console.error('This Bun version cannot be trusted to report failures (see `bun --version`).');
		process.exit(2);
	}
}
console.log('\ncanaries failed as expected; the runner reports failures');
if (canaryOnly) process.exit(0);

const roots = targets.length
	? targets.map((t) => resolve(t))
	: [join(root, 'tests'), join(root, 'src')];
const files = roots.flatMap((r) => find(r));
const failed = [];
for (const file of files) {
	console.log(`\n# ${relative(root, file)}`);
	if (run(file) !== 0) failed.push(relative(root, file));
}
console.log(
	`\n${files.length} test files, ${files.length - failed.length} passed, ${failed.length} failed`
);
for (const file of failed) console.log(`FAILED ${file}`);
process.exit(failed.length > 0 || files.length === 0 ? 1 : 0);
