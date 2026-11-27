/**
 * Conformance runner for the client rule engine: `bun scripts/cel-conformance.ts <fixtures.json>`
 * (run from `frontend/`). Prints `<id> <verdict>` per case in the format of the Rust runner
 * (`cargo run -p sv-rules --example conformance -- <fixtures.json>`); `diff` of the two outputs
 * must be empty. Cases whose verdict differs from the fixture's `expect` are listed on stderr and
 * make the exit status 1.
 */

import { readFileSync } from 'node:fs';

import {
	checkBuiltin,
	compile,
	parseRule,
	RULE_KINDS,
	type CelEnv,
	type Operand,
	type RuleKind
} from '../src/lib/forms/rules/index.ts';

type Json = null | boolean | number | string | Json[] | { [k: string]: Json };
type Case = Record<string, Json>;

function jsNumber(f: number): string {
	if (Number.isNaN(f)) return 'NaN';
	if (f === Infinity) return '+Inf';
	if (f === -Infinity) return '-Inf';
	return `${f}`;
}

function canonicalKey(k: unknown): string {
	if (typeof k === 'bigint') return `int:${k}`;
	if (typeof k === 'boolean') return `${k}`;
	return `string:${JSON.stringify(String(k))}`;
}

function canonicalCel(v: unknown): string {
	if (v === null || v === undefined) return 'null';
	if (typeof v === 'boolean') return `${v}`;
	if (typeof v === 'bigint') return `int:${v}`;
	if (typeof v === 'number') return `double:${jsNumber(v)}`;
	if (typeof v === 'string') return `string:${JSON.stringify(v)}`;
	if (Array.isArray(v)) return `[${v.map(canonicalCel).join(',')}]`;
	if (v instanceof Map) {
		const entries = [...v.entries()].map(([k, val]) => `${canonicalKey(k)}:${canonicalCel(val)}`);
		return `{${entries.sort().join(',')}}`;
	}
	if (typeof v === 'object') {
		const entries = Object.entries(v).map(([k, val]) => `${canonicalKey(k)}:${canonicalCel(val)}`);
		return `{${entries.sort().join(',')}}`;
	}
	return `unsupported:${String(v)}`;
}

/** The canonical text of an expected JSON value, read with the binding rule. */
function canonicalJson(v: Json): string {
	if (v === null) return 'null';
	if (typeof v === 'boolean') return `${v}`;
	if (typeof v === 'number') {
		return Number.isSafeInteger(v) ? `int:${v}` : `double:${jsNumber(v)}`;
	}
	if (typeof v === 'string') return `string:${JSON.stringify(v)}`;
	if (Array.isArray(v)) return `[${v.map(canonicalJson).join(',')}]`;
	const entries = Object.entries(v).map(([k, val]) => `${canonicalKey(k)}:${canonicalJson(val)}`);
	return `{${entries.sort().join(',')}}`;
}

function obj(v: Json | undefined): Record<string, Json> {
	return typeof v === 'object' && v !== null && !Array.isArray(v) ? v : {};
}

function exprCase(c: Case): [string, string] {
	const env = (typeof c.env === 'string' ? c.env : 'field_rule') as CelEnv;
	const src = typeof c.expr === 'string' ? c.expr : '';
	const compiled = compile(src, env);
	let got: string;
	if (!compiled.ok) {
		got = `compile_error=${compiled.code}`;
	} else {
		const r = compiled.program.eval(obj(c.vars));
		got = r.ok ? `value=${canonicalCel(r.value)}` : `eval_error=${r.code}`;
	}
	const e = obj(c.expect);
	let want = 'missing-expect';
	if ('value' in e) want = `value=${canonicalJson(e.value)}`;
	else if ('compile_error' in e) want = `compile_error=${e.compile_error}`;
	else if ('eval_error' in e) want = `eval_error=${e.eval_error}`;
	return [got, want];
}

function operand(v: Json | undefined): Operand | null {
	const entries = Object.entries(obj(v));
	if (entries.length === 0) return null;
	const [kind, value] = entries[0];
	switch (kind) {
		case 'int':
			return typeof value === 'number' ? { kind, value: BigInt(value) } : null;
		case 'count':
			return typeof value === 'number' ? { kind, value } : null;
		case 'decimal':
		case 'date':
		case 'datetime':
		case 'str':
			return typeof value === 'string' ? { kind, value } : null;
	}
	return null;
}

function ruleCase(c: Case): [string, string] {
	const allowed: readonly RuleKind[] = Array.isArray(c.allowed)
		? RULE_KINDS.filter((k) => (c.allowed as Json[]).includes(k))
		: RULE_KINDS;
	const parsed = parseRule(c.rule, allowed);
	let got: string;
	if (!parsed.ok) {
		got = `def_error=${parsed.error.code}@${parsed.error.path}`;
	} else if (parsed.rule.spec.type !== 'builtin') {
		got = 'parsed';
	} else {
		const op = operand(c.operand);
		if (op === null) {
			got = 'bad-operand';
		} else {
			const code = checkBuiltin(parsed.rule.spec.rule, op);
			got = code === null ? 'pass' : `fail=${code}`;
		}
	}
	const e = obj(c.expect);
	let want = 'missing-expect';
	if ('pass' in e) want = 'pass';
	else if ('parsed' in e) want = 'parsed';
	else if ('fail' in e) want = `fail=${e.fail}`;
	else if ('def_error' in e) want = `def_error=${e.def_error}@${e.path ?? ''}`;
	return [got, want];
}

function main(): number {
	const path = process.argv[2];
	if (!path) {
		process.stderr.write('usage: bun scripts/cel-conformance.ts <fixtures.json>\n');
		return 2;
	}
	const fixtures = JSON.parse(readFileSync(path, 'utf8')) as { cases?: Case[] };
	const cases = fixtures.cases ?? [];
	let mismatches = 0;
	const out: string[] = [];
	for (const c of cases) {
		const id = typeof c.id === 'string' ? c.id : '?';
		const [got, want] = 'rule' in c ? ruleCase(c) : exprCase(c);
		out.push(`${id} ${got}`);
		if (got !== want) {
			mismatches++;
			process.stderr.write(`MISMATCH ${id}: got ${got}, expected ${want}\n`);
		}
	}
	process.stdout.write(out.join('\n') + '\n');
	process.stderr.write(`${cases.length} cases, ${mismatches} differ from expect\n`);
	return mismatches === 0 ? 0 : 1;
}

process.exit(main());
