#!/usr/bin/env node
// Line counts of the forms page and every file the split creates.
//
//   node scripts/forms-split/line-budget.mjs [--final]
//
// Exit 1 when any file is above the hard limit (3,000 lines). With --final, files above the
// budgets (route 600, components 800, .ts modules 600) are listed as warnings.
// With --allow-route, the route file is exempt from the hard limit (allowed between W0-C and
// the end of wave 1) and is reported as such.
import { readdirSync, readFileSync, statSync, existsSync } from 'node:fs';
import { join, relative } from 'node:path';

const root = new URL('../../', import.meta.url).pathname;
const HARD_LIMIT = 3000;
const folders = [
	'src/routes/(app)/workspace/[workspaceId]/projects/[projectId]/forms',
	'src/lib/forms',
	'src/lib/components/forms/page',
	'src/lib/components/forms/views'
];
const final = process.argv.includes('--final');
const allowRoute = process.argv.includes('--allow-route');

function walk(dir, found = []) {
	if (!existsSync(dir)) return found;
	for (const name of readdirSync(dir)) {
		const path = join(dir, name);
		if (statSync(path).isDirectory()) walk(path, found);
		else found.push(path);
	}
	return found;
}

function budget(file) {
	if (file.endsWith('+page.svelte')) return 600;
	if (file.endsWith('.svelte')) return 800;
	return 600;
}

function lineCount(file) {
	const text = readFileSync(file, 'utf8');
	if (text === '') return 0;
	return text.split('\n').length - (text.endsWith('\n') ? 1 : 0);
}

let over = 0;
let warnings = 0;
const rows = [];
for (const folder of folders) {
	for (const file of walk(join(root, folder)).sort()) {
		const lines = lineCount(file);
		const rel = relative(root, file);
		let note = '';
		if (lines > HARD_LIMIT && allowRoute && file.endsWith('+page.svelte')) {
			note = 'route exempt (--allow-route)';
		} else if (lines > HARD_LIMIT) {
			note = 'OVER HARD LIMIT';
			over += 1;
		} else if (final && lines > budget(file)) {
			note = `warning: above budget ${budget(file)}`;
			warnings += 1;
		}
		rows.push({ rel, lines, note });
	}
}
for (const row of rows)
	console.log(`${String(row.lines).padStart(7)}  ${row.rel}${row.note ? `  [${row.note}]` : ''}`);
console.log(
	`\n${rows.length} files, ${over} above ${HARD_LIMIT} lines${final ? `, ${warnings} above budget` : ''}`
);
process.exit(over > 0 ? 1 : 0);
