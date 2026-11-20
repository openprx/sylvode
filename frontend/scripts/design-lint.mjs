#!/usr/bin/env node
// Design lint, report mode. Counts, per file, the legacy constructs the redesign removes:
//   palette   raw palette utilities (slate, blue, red, ... with a numeric shade)
//   dark      `dark:` variants
//   hex       literal hex colours
//   rounded   rounded-sm, rounded-md, rounded-lg, rounded-xl (any variant prefix)
//   confirm   native window.confirm calls
//   toast     toast.<method>( calls
//
//   node scripts/design-lint.mjs [--root src] [--out reports/design-lint] [--top 10] [--gate]
//
// Writes <out>.json and <out>.md, files ordered by total, largest first. Exit code is 0 unless
// --gate is given and any count is non-zero (the later CI gate).

import { mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, join, relative, resolve } from 'node:path';

export const RULES = {
	palette:
		/(?<![\w-])(?:[\w[\]&>*.=-]+:)*-?(?:bg|text|border|divide|ring|ring-offset|outline|fill|stroke|from|via|to|placeholder|decoration|accent|caret|shadow)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}(?:\/\d+)?(?![\w-])/g,
	dark: /(?<![\w-])dark:/g,
	hex: /(?<![\w&#/.:-])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w-])/g,
	rounded: /(?<![\w-])rounded-(?:sm|md|lg|xl)(?![\w-])/g,
	confirm: /(?<![\w.$])(?:window\.)?confirm\s*\(/g,
	toast: /(?<![\w.$])toast\.[a-zA-Z]+\s*\(/g
};
const KEYS = Object.keys(RULES);
const EXTENSIONS = new Set(['.svelte', '.ts', '.js', '.css', '.html']);
// The token file is the source of the colours, not a use of them.
const SKIP_FILES = new Set(['tokens.css']);

/** `confirm` is counted in Svelte files only: in TypeScript it also names methods and comments. */
export function countSource(text, file = '.svelte') {
	const counts = {};
	for (const key of KEYS) {
		counts[key] =
			key === 'confirm' && !file.endsWith('.svelte') ? 0 : (text.match(RULES[key]) ?? []).length;
	}
	counts.total = KEYS.reduce((sum, key) => sum + counts[key], 0);
	return counts;
}

async function walk(dir, files = []) {
	for (const entry of await readdir(dir, { withFileTypes: true })) {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) await walk(path, files);
		else if (
			EXTENSIONS.has(entry.name.slice(entry.name.lastIndexOf('.'))) &&
			!SKIP_FILES.has(entry.name)
		)
			files.push(path);
	}
	return files;
}

export async function lintTree(root) {
	const rows = [];
	for (const file of await walk(root)) {
		const counts = countSource(await readFile(file, 'utf8'), file);
		if (counts.total > 0) rows.push({ file: relative(root, file), ...counts });
	}
	rows.sort((a, b) => b.total - a.total || a.file.localeCompare(b.file));
	const totals = { total: 0 };
	for (const key of KEYS) totals[key] = rows.reduce((sum, row) => sum + row[key], 0);
	totals.total = KEYS.reduce((sum, key) => sum + totals[key], 0);
	return { rows, totals };
}

function markdown(root, { rows, totals }) {
	const head = `| File | total | ${KEYS.join(' | ')} |`;
	const rule = `|---|---:|${KEYS.map(() => '---:').join('|')}|`;
	const lines = [
		'# Design lint report',
		'',
		`Root: \`${root}\`. Files with at least one hit: ${rows.length}. Largest first.`,
		'',
		`Totals: ${['total', ...KEYS].map((key) => `${key} ${totals[key]}`).join(', ')}.`,
		'',
		head,
		rule
	];
	for (const row of rows)
		lines.push(`| ${row.file} | ${row.total} | ${KEYS.map((key) => row[key]).join(' | ')} |`);
	return lines.join('\n') + '\n';
}

async function main() {
	const args = process.argv.slice(2);
	const flag = (name, fallback) => {
		const index = args.indexOf(`--${name}`);
		return index >= 0 && args[index + 1] && !args[index + 1].startsWith('--')
			? args[index + 1]
			: fallback;
	};
	const root = resolve(flag('root', 'src'));
	const out = resolve(flag('out', 'reports/design-lint'));
	const top = Number(flag('top', 10));
	const result = await lintTree(root);
	await mkdir(dirname(out), { recursive: true });
	await writeFile(`${out}.json`, JSON.stringify(result, null, 2) + '\n');
	await writeFile(`${out}.md`, markdown(root, result));
	console.log(
		`design-lint: ${result.rows.length} files with hits, ${result.totals.total} in total`
	);
	console.log(`  ${KEYS.map((key) => `${key} ${result.totals[key]}`).join(', ')}`);
	for (const row of result.rows.slice(0, top))
		console.log(`  ${String(row.total).padStart(6)}  ${row.file}`);
	console.log(`report: ${out}.md, ${out}.json`);
	if (args.includes('--gate') && result.totals.total > 0) process.exitCode = 1;
}

if (import.meta.url === `file://${process.argv[1]}`) {
	main().catch((error) => {
		console.error(`design-lint: ${error.message}`);
		process.exit(1);
	});
}
