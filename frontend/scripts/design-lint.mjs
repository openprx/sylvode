#!/usr/bin/env node
// Design lint, report mode. Counts, per file, the legacy constructs the redesign removes:
//   palette   raw palette utilities (slate, blue, red, ... with a numeric shade)
//   dark      `dark:` variants
//   hex       literal hex colours
//   rounded   rounded-sm, rounded-md, rounded-lg, rounded-xl (any variant prefix)
//   confirm   native window.confirm calls
//   toast     toast.<method>( calls
//
//   node scripts/design-lint.mjs [--root src [--root dir-or-file ...]] [--out reports/design-lint] [--top 10] [--gate]
//                                [--rules palette,dark,hex,rounded] [--allow scripts/design-lint-allow.json]
//
// Writes <out>.json and <out>.md, files ordered by total, largest first. Exit code is 0 unless
// --gate is given and any counted number is non-zero (the later CI gate).
//
// --root           A directory or a single file; repeat it to lint several, one section each.
// --rules <list>   Only these categories are counted (default: all). The gate looks at them only.
// --allow <file>   Per-file, per-category exceptions: a JSON array of
//                  { "file": "src/lib/shell/example.ts", "rules": ["hex"], "reason": "..." }
//                  `file` is relative to the working directory (the frontend folder), `reason`
//                  is required. An allowed category of an allowed file is not counted. An entry
//                  that matched nothing (file missing, or the category has no hits any more) is
//                  reported as stale and fails the gate, so the list cannot rot.

import { mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { dirname, join, normalize, relative, resolve, sep } from 'node:path';

export const RULES = {
	palette:
		/(?<![\w-])(?:[\w[\]&>*.=-]+:)*-?(?:bg|text|border|divide|ring|ring-offset|outline|fill|stroke|from|via|to|placeholder|decoration|accent|caret|shadow)-(?:slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose)-\d{2,3}(?:\/\d+)?(?![\w-])/g,
	// A variant is followed at once by the next token; `dark: [` or `dark: 1` is an object key.
	dark: /(?<![\w-])dark:(?=[^\s'"`,;)\]}])/g,
	hex: /(?<![\w&#/.:-])#(?:[0-9a-fA-F]{8}|[0-9a-fA-F]{6}|[0-9a-fA-F]{3,4})(?![\w-])/g,
	rounded: /(?<![\w-])rounded-(?:sm|md|lg|xl)(?![\w-])/g,
	confirm: /(?<![\w.$])(?:window\.)?confirm\s*\(/g,
	toast: /(?<![\w.$])toast\.[a-zA-Z]+\s*\(/g
};
const ALL_KEYS = Object.keys(RULES);
const EXTENSIONS = new Set(['.svelte', '.ts', '.js', '.css', '.html']);
// The token file is the source of the colours, not a use of them.
const SKIP_FILES = new Set(['tokens.css']);

/** `confirm` is counted in Svelte files only: in TypeScript it also names methods and comments. */
export function countSource(text, file = '.svelte', keys = ALL_KEYS) {
	const counts = {};
	for (const key of keys) {
		counts[key] =
			key === 'confirm' && !file.endsWith('.svelte') ? 0 : (text.match(RULES[key]) ?? []).length;
	}
	counts.total = keys.reduce((sum, key) => sum + counts[key], 0);
	return counts;
}

/** `palette,dark` to ['palette', 'dark']; throws on an unknown category. */
export function parseRules(value) {
	if (value === undefined || value === true) return ALL_KEYS;
	const keys = String(value)
		.split(',')
		.map((key) => key.trim())
		.filter(Boolean);
	const unknown = keys.filter((key) => !ALL_KEYS.includes(key));
	if (unknown.length || keys.length === 0)
		throw new Error(
			`--rules: unknown category ${unknown.join(', ') || '(empty)'}; use ${ALL_KEYS.join(', ')}`
		);
	return ALL_KEYS.filter((key) => keys.includes(key));
}

/** Validates the allowlist entries. Throws on a missing file, an unknown category or no reason. */
export function parseAllow(entries) {
	if (!Array.isArray(entries)) throw new Error('--allow: the file must hold a JSON array');
	return entries.map((entry, index) => {
		const where = `--allow entry ${index + 1}`;
		if (!entry || typeof entry.file !== 'string' || !entry.file)
			throw new Error(`${where}: "file" is required`);
		if (typeof entry.reason !== 'string' || !entry.reason.trim())
			throw new Error(`${where} (${entry.file}): "reason" is required`);
		if (!Array.isArray(entry.rules) || entry.rules.length === 0)
			throw new Error(`${where} (${entry.file}): "rules" must list at least one category`);
		const unknown = entry.rules.filter((key) => !ALL_KEYS.includes(key));
		if (unknown.length)
			throw new Error(`${where} (${entry.file}): unknown category ${unknown.join(', ')}`);
		return { file: normalize(entry.file), rules: entry.rules, reason: entry.reason.trim() };
	});
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

/**
 * Counts every file under `root` (a directory or one file). With `allow`, the allowed categories of
 * the allowed files are not counted. `stale` lists the allowed categories, for files inside this
 * root, that excused nothing: the file is gone or the category has no hits any more. Entries for
 * files outside the root, and categories not selected in `rules`, are ignored.
 */
export async function lintTree(root, { rules = ALL_KEYS, allow = [], cwd = process.cwd() } = {}) {
	const rows = [];
	const hit = new Set();
	const files = (await stat(root)).isDirectory() ? await walk(root) : [root];
	const allowedFor = (file) => allow.filter((entry) => resolve(cwd, entry.file) === file);
	for (const file of files) {
		const counts = countSource(await readFile(file, 'utf8'), file, rules);
		for (const entry of allowedFor(file)) {
			for (const key of entry.rules) {
				if (!rules.includes(key)) continue;
				if (counts[key] > 0) hit.add(`${entry.file}\0${key}`);
				counts.total -= counts[key];
				counts[key] = 0;
			}
		}
		if (counts.total > 0)
			rows.push({ file: relative(root, file) || relative(cwd, file), ...counts });
	}
	rows.sort((a, b) => b.total - a.total || a.file.localeCompare(b.file));
	const totals = { total: 0 };
	for (const key of rules) totals[key] = rows.reduce((sum, row) => sum + row[key], 0);
	totals.total = rules.reduce((sum, key) => sum + totals[key], 0);
	const inside = (path) => path === root || path.startsWith(root + sep);
	const stale = [];
	for (const entry of allow) {
		if (!inside(resolve(cwd, entry.file))) continue;
		for (const key of entry.rules)
			if (rules.includes(key) && !hit.has(`${entry.file}\0${key}`))
				stale.push({ file: entry.file, rule: key, reason: entry.reason });
	}
	return { rows, totals, stale, rules };
}

function markdown(root, { rows, totals, stale, rules }) {
	const head = `| File | total | ${rules.join(' | ')} |`;
	const rule = `|---|---:|${rules.map(() => '---:').join('|')}|`;
	const lines = [
		'# Design lint report',
		'',
		`Root: \`${root}\`. Rules: ${rules.join(', ')}. Files with at least one hit: ${rows.length}. Largest first.`,
		'',
		`Totals: ${['total', ...rules].map((key) => `${key} ${totals[key]}`).join(', ')}.`,
		'',
		head,
		rule
	];
	for (const row of rows)
		lines.push(`| ${row.file} | ${row.total} | ${rules.map((key) => row[key]).join(' | ')} |`);
	if (stale.length) {
		lines.push('', '## Stale allowlist entries', '');
		for (const entry of stale) lines.push(`- \`${entry.file}\` ${entry.rule}: ${entry.reason}`);
	}
	return lines.join('\n') + '\n';
}

/** Every value given to a repeatable flag: `--root a --root b`. */
export function flagValues(args, name) {
	const values = [];
	args.forEach((arg, index) => {
		const next = args[index + 1];
		if (arg === `--${name}` && next && !next.startsWith('--')) values.push(next);
	});
	return values;
}

async function main() {
	const args = process.argv.slice(2);
	const flag = (name, fallback) => flagValues(args, name)[0] ?? fallback;
	const roots = flagValues(args, 'root').map((root) => resolve(root));
	if (roots.length === 0) roots.push(resolve('src'));
	const out = resolve(flag('out', 'reports/design-lint'));
	const top = Number(flag('top', 10));
	const rules = parseRules(flag('rules', undefined));
	const allowFile = flag('allow', undefined);
	const allow = allowFile ? parseAllow(JSON.parse(await readFile(resolve(allowFile), 'utf8'))) : [];
	const results = [];
	for (const root of roots) results.push({ root, ...(await lintTree(root, { rules, allow })) });
	await mkdir(dirname(out), { recursive: true });
	// One root keeps the original report shape; several roots write one report per root section.
	await writeFile(
		`${out}.json`,
		JSON.stringify(results.length === 1 ? results[0] : { roots: results }, null, 2) + '\n'
	);
	await writeFile(`${out}.md`, results.map((r) => markdown(r.root, r)).join('\n'));
	let failing = false;
	for (const result of results) {
		const label = relative(process.cwd(), result.root) || result.root;
		if (results.length > 1) console.log(`${label}`);
		console.log(
			`design-lint: ${result.rows.length} files with hits, ${result.totals.total} in total`
		);
		console.log(`  ${rules.map((key) => `${key} ${result.totals[key]}`).join(', ')}`);
		for (const row of result.rows.slice(0, top))
			console.log(`  ${String(row.total).padStart(6)}  ${row.file}`);
		for (const entry of result.stale)
			console.log(`  STALE allowlist entry: ${entry.file} ${entry.rule} (${entry.reason})`);
		if (result.totals.total > 0 || result.stale.length > 0) failing = true;
	}
	console.log(`report: ${out}.md, ${out}.json`);
	if (args.includes('--gate') && failing) process.exitCode = 1;
}

if (import.meta.url === `file://${process.argv[1]}`) {
	main().catch((error) => {
		console.error(`design-lint: ${error.message}`);
		process.exit(1);
	});
}
