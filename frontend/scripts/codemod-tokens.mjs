#!/usr/bin/env node
// Token codemod: the "safe to script" rows of the design specification (7.6).
//
//   node scripts/codemod-tokens.mjs [--write] [path ...]     (default path: src; dry run unless --write)
//
// Rules (all work on class lists, never on prose or code):
//   1. pairs     a light utility and its `dark:` twin (same variants) collapse into one token utility:
//                text, border and divide pairs of the slate scale, and text pairs of red, amber, emerald, green.
//   2. focus     delete focus ring utilities: focus:/focus-visible: ring, ring-offset, outline (and
//                `dark:` twins), and focus:border-blue-*.
//   3. shadow    delete `shadow-sm` and `dark:shadow-*`.
//   4. bold      font-bold -> font-semibold (unread counts are restored by hand afterwards).
//
// A class list is a run of whitespace-separated tokens bounded by quotes, braces, angle brackets,
// `=` or a backtick. Pairs are collapsed only inside one run. Anything that matches no rule is left
// byte for byte as it was. Backgrounds, blue and cyan, text-sm and radii are NOT touched (by hand).

import { readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';

const range = (prefix, shades) => shades.map((shade) => `${prefix}-${shade}`);

/** Pair table: `target` replaces a light utility that has a dark twin from `dark`. */
const PAIR_RULES = [
	{
		target: 'text-fg',
		light: ['text-slate-900', 'text-slate-950', 'text-slate-800', 'text-slate-700'],
		dark: ['text-slate-100', 'text-slate-200', 'text-slate-300']
	},
	{
		target: 'text-fg-muted',
		light: ['text-slate-600', 'text-slate-500', 'text-slate-400'],
		dark: ['text-slate-400', 'text-slate-300']
	},
	{
		target: 'border-line',
		light: ['border-slate-200', 'border-slate-100'],
		dark: ['border-slate-700', 'border-slate-800']
	},
	{
		target: 'divide-line',
		light: ['divide-slate-200', 'divide-slate-100'],
		dark: ['divide-slate-700', 'divide-slate-800']
	},
	{
		target: 'text-danger',
		light: range('text-red', [600, 700, 800]),
		dark: range('text-red', [200, 300, 400])
	},
	{
		target: 'text-warning',
		light: range('text-amber', [600, 700, 800, 900]),
		dark: range('text-amber', [200, 300, 400])
	},
	{
		target: 'text-success',
		light: [...range('text-emerald', [600, 700, 800]), ...range('text-green', [600, 700, 800])],
		dark: [...range('text-emerald', [300, 400]), ...range('text-green', [300, 400])]
	}
];

const RUN = /[^\s"'`{}<>=]+(?:[ \t\r\n]+[^\s"'`{}<>=]+)*/g;

/** Split `a:b:[&>x]:util` into variants and utility, ignoring colons inside brackets. */
function splitToken(token) {
	const parts = [];
	let depth = 0;
	let start = 0;
	for (let i = 0; i < token.length; i += 1) {
		const ch = token[i];
		if (ch === '[' || ch === '(') depth += 1;
		else if (ch === ']' || ch === ')') depth -= 1;
		else if (ch === ':' && depth === 0) {
			parts.push(token.slice(start, i));
			start = i + 1;
		}
	}
	return { variants: parts, utility: token.slice(start) };
}

const join_ = (variants, utility) => [...variants, utility].join(':');

function isFocusRing({ variants, utility }) {
	const rest = variants[0] === 'dark' ? variants.slice(1) : variants;
	if (rest.length !== 1 || (rest[0] !== 'focus' && rest[0] !== 'focus-visible')) return false;
	return /^(ring|ring-.+|outline|outline-.+)$/.test(utility);
}

function isFocusBorderBlue({ variants, utility }) {
	const rest = variants[0] === 'dark' ? variants.slice(1) : variants;
	return rest.length === 1 && rest[0] === 'focus' && /^border-blue-\d{2,3}$/.test(utility);
}

/** Transform one run of class tokens. Returns the new run text, or null when unchanged. */
function transformRun(run, counts) {
	const pieces = run.split(/([ \t\r\n]+)/);
	// pieces alternate token, separator, token, ...
	const tokens = [];
	for (let i = 0; i < pieces.length; i += 2)
		tokens.push({ text: pieces[i], sep: pieces[i + 1] ?? '' });
	const parsed = tokens.map((t) => splitToken(t.text));
	const present = new Set(tokens.map((t) => t.text));
	const removed = new Array(tokens.length).fill(false);
	const replaced = new Array(tokens.length).fill(null);
	let changed = false;

	tokens.forEach((token, i) => {
		const { variants, utility } = parsed[i];
		if (isFocusRing(parsed[i]) || isFocusBorderBlue(parsed[i])) {
			removed[i] = true;
			counts.focus += 1;
			return;
		}
		if (
			(variants.length === 0 && utility === 'shadow-sm') ||
			(variants.length === 1 && variants[0] === 'dark' && /^shadow-.+/.test(utility))
		) {
			removed[i] = true;
			counts.shadow += 1;
			return;
		}
		if (utility === 'font-bold' && !variants.includes('dark')) {
			replaced[i] = join_(variants, 'font-semibold');
			counts.bold += 1;
			return;
		}
		if (variants[0] === 'dark') return;
		for (const rule of PAIR_RULES) {
			if (!rule.light.includes(utility)) continue;
			const twin = rule.dark.find((d) => present.has(join_(['dark', ...variants], d)));
			if (!twin) {
				counts.unpaired += 1;
				return;
			}
			replaced[i] = join_(variants, rule.target);
			counts.pairs += 1;
			// delete every dark twin of this rule that sits in the run
			tokens.forEach((other, j) => {
				const o = parsed[j];
				if (
					!removed[j] &&
					o.variants[0] === 'dark' &&
					o.variants.length === variants.length + 1 &&
					variants.every((v, k) => o.variants[k + 1] === v) &&
					rule.dark.includes(o.utility)
				)
					removed[j] = true;
			});
			return;
		}
	});

	const keep = [];
	const seen = new Set();
	tokens.forEach((token, i) => {
		if (removed[i]) {
			changed = true;
			return;
		}
		const text = replaced[i] ?? token.text;
		if (replaced[i] !== null) changed = true;
		if (replaced[i] !== null && seen.has(text)) {
			changed = true;
			return;
		}
		seen.add(text);
		keep.push({ text, sep: token.sep });
	});
	if (!changed) return null;
	if (keep.length === 0) return '';
	// keep the original separators between surviving tokens; the last one ends the run
	return keep.map((t, k) => (k < keep.length - 1 ? t.text + (t.sep || ' ') : t.text)).join('');
}

export function transformSource(source) {
	const counts = { pairs: 0, focus: 0, shadow: 0, bold: 0, unpaired: 0 };
	const text = source.replace(RUN, (run) => {
		// prose and code rarely start with a class-like token; only runs with a candidate are rewritten
		if (!/(text-|border-|divide-|ring|outline|shadow-|font-bold)/.test(run)) return run;
		const next = transformRun(run, counts);
		return next === null ? run : next;
	});
	return { text, counts };
}

async function walk(path, files = []) {
	const info = await stat(path);
	if (info.isFile()) {
		if (path.endsWith('.svelte')) files.push(path);
		return files;
	}
	for (const entry of await readdir(path, { withFileTypes: true }))
		await walk(join(path, entry.name), files);
	return files;
}

async function main() {
	const args = process.argv.slice(2);
	const write = args.includes('--write');
	const paths = args.filter((a) => !a.startsWith('--'));
	const roots = (paths.length ? paths : ['src']).map((p) => resolve(p));
	const total = { pairs: 0, focus: 0, shadow: 0, bold: 0, unpaired: 0 };
	let filesChanged = 0;
	for (const root of roots) {
		for (const file of await walk(root)) {
			const source = await readFile(file, 'utf8');
			const { text, counts } = transformSource(source);
			for (const key of Object.keys(total)) total[key] += counts[key];
			if (text !== source) {
				filesChanged += 1;
				if (write) await writeFile(file, text);
			}
		}
	}
	console.log(
		`codemod-tokens (${write ? 'written' : 'dry run'}): ${filesChanged} files change; ` +
			Object.entries(total)
				.map(([k, v]) => `${k} ${v}`)
				.join(', ')
	);
}

if (import.meta.url === `file://${process.argv[1]}`) {
	main().catch((error) => {
		console.error(`codemod-tokens: ${error.message}`);
		process.exit(1);
	});
}
