/**
 * The regex subset of the `pattern` rule and CEL `matches`; mirror of
 * `crates/sv-rules/src/regex_subset.rs` (read its module docs for the rules). The scanner is
 * written the same way on both sides; the browser then compiles the original pattern with
 * `new RegExp(p, 'u' + (ci ? 'i' : ''))`.
 */

import {
	CLASS_UNITS,
	MAX_PATTERN_BYTES,
	MAX_REPEAT,
	MAX_REPEAT_UNITS,
	utf8Length
} from './limits.ts';

export type PatternCode = 'pattern_unsupported' | 'pattern_invalid' | 'rule_param_invalid';

const UNSUPPORTED = 'pattern_unsupported';
const INVALID = 'pattern_invalid';
const SYNTAX_CHARS = '^$\\.*+?()[]{}|/';
const CLASS_ESCAPES = 'dwsDWS';

class Fail {
	constructor(readonly code: PatternCode) {}
}

type ClassItem = { char: number } | { set: true };

class Scanner {
	private readonly cps: number[];
	private i = 0;

	constructor(pattern: string) {
		this.cps = Array.from(pattern, (c) => c.codePointAt(0) ?? 0);
	}

	private next(): number | undefined {
		return this.cps[this.i++];
	}

	private peek(offset = 0): number | undefined {
		return this.cps[this.i + offset];
	}

	private hex(n: number): number {
		let v = 0;
		for (let k = 0; k < n; k++) {
			const c = this.next();
			const d = c === undefined ? NaN : parseInt(String.fromCodePoint(c), 16);
			if (Number.isNaN(d)) throw new Fail(UNSUPPORTED);
			v = v * 16 + d;
		}
		return v;
	}

	private escape(inClass: boolean): ClassItem {
		const cp = this.next();
		if (cp === undefined) throw new Fail(INVALID);
		const c = String.fromCodePoint(cp);
		if (CLASS_ESCAPES.includes(c)) return { set: true };
		switch (c) {
			case 't':
				return { char: 0x09 };
			case 'n':
				return { char: 0x0a };
			case 'v':
				return { char: 0x0b };
			case 'f':
				return { char: 0x0c };
			case 'r':
				return { char: 0x0d };
			case 'x':
				return { char: this.hex(2) };
			case 'u': {
				const v = this.hex(4);
				if (v >= 0xd800 && v <= 0xdfff) throw new Fail(UNSUPPORTED);
				return { char: v };
			}
		}
		if (SYNTAX_CHARS.includes(c) || (inClass && c === '-')) return { char: cp };
		throw new Fail(UNSUPPORTED);
	}

	private static literal(cp: number): number {
		if (cp > 0xffff || (cp >= 0xd800 && cp <= 0xdfff)) throw new Fail(UNSUPPORTED);
		return cp;
	}

	private klass(): void {
		if (this.peek() === 0x5e /* ^ */) this.next();
		if (this.peek() === 0x5d /* ] */) throw new Fail(UNSUPPORTED);
		let members = 0;
		for (;;) {
			const cp = this.next();
			if (cp === undefined) throw new Fail(INVALID);
			if (cp === 0x5d) break;
			if (cp === 0x5b /* [ */) throw new Fail(UNSUPPORTED);
			const item: ClassItem = cp === 0x5c ? this.escape(true) : { char: Scanner.literal(cp) };
			members++;
			const isRange =
				this.peek() === 0x2d /* - */ && this.peek(1) !== undefined && this.peek(1) !== 0x5d;
			if (!isRange) continue;
			this.next();
			const hiCp = this.next();
			if (hiCp === undefined) throw new Fail(INVALID);
			if (hiCp === 0x5b) throw new Fail(UNSUPPORTED);
			const hi: ClassItem = hiCp === 0x5c ? this.escape(true) : { char: Scanner.literal(hiCp) };
			if (!('char' in item) || !('char' in hi)) throw new Fail(INVALID);
			if (item.char > hi.char) throw new Fail(INVALID);
		}
		if (members === 0) throw new Fail(UNSUPPORTED);
	}

	private number(): number | null {
		let digits = '';
		while (this.peek() !== undefined && /[0-9]/.test(String.fromCodePoint(this.peek()!))) {
			digits += String.fromCodePoint(this.next()!);
		}
		if (digits === '') return null;
		const n = Number(digits);
		return n > 0xffffffff ? 0xffffffff : n;
	}

	/** Parses `{n}`, `{n,}`, `{n,m}` and returns the multiplier (`m`, or `n` when open; at least 1). */
	private braces(): number {
		const lo = this.number();
		if (lo === null) throw new Fail(INVALID);
		let hi: number | null = lo;
		if (this.peek() === 0x2c /* , */) {
			this.next();
			hi = this.number();
		}
		if (this.next() !== 0x7d /* } */) throw new Fail(INVALID);
		if (lo > MAX_REPEAT || (hi !== null && hi > MAX_REPEAT)) throw new Fail(UNSUPPORTED);
		if (hi !== null && hi < lo) throw new Fail(INVALID);
		return Math.max(hi ?? lo, 1);
	}

	run(): void {
		// Size units per open group: a literal costs 1, a class, class escape or `.` costs
		// CLASS_UNITS, and `{n,m}` multiplies its atom by `m` (by `n` when open).
		const frames: number[] = [0];
		// Units of the last atom, the operand of a following quantifier.
		let atom: number | null = null;
		const total = () => frames.reduce((a, b) => a + b, 0);
		for (let cp = this.next(); cp !== undefined; cp = this.next()) {
			const c = String.fromCodePoint(cp);
			let units: number;
			switch (c) {
				case '\\':
					units = 'set' in this.escape(false) ? CLASS_UNITS : 1;
					break;
				case '[':
					this.klass();
					units = CLASS_UNITS;
					break;
				case '(':
					if (this.peek() === 0x3f /* ? */) {
						this.next();
						if (this.next() !== 0x3a /* : */) throw new Fail(UNSUPPORTED);
					}
					frames.push(0);
					atom = null;
					continue;
				case ')':
					if (frames.length === 1) throw new Fail(INVALID);
					units = frames.pop()!;
					break;
				case '*':
				case '+':
				case '?':
				case '{': {
					if (atom === null) throw new Fail(INVALID);
					const bound = c === '{' ? this.braces() : 1;
					frames[frames.length - 1] += atom * (bound - 1);
					if (this.peek() === 0x3f) this.next();
					atom = null;
					if (total() > MAX_REPEAT_UNITS) throw new Fail(UNSUPPORTED);
					continue;
				}
				case ']':
				case '}':
					throw new Fail(UNSUPPORTED);
				case '^':
				case '$':
				case '|':
					atom = null;
					continue;
				case '.':
					units = CLASS_UNITS;
					break;
				default:
					Scanner.literal(cp);
					units = 1;
			}
			frames[frames.length - 1] += units;
			atom = units;
		}
		if (frames.length !== 1) throw new Fail(INVALID);
		if (total() > MAX_REPEAT_UNITS) throw new Fail(UNSUPPORTED);
	}
}

/** Checks a pattern against the subset and compiles it; returns the regex or the refusal code. */
export function compilePattern(pattern: string, caseInsensitive = false): RegExp | PatternCode {
	if (utf8Length(pattern) > MAX_PATTERN_BYTES) return 'rule_param_invalid';
	try {
		new Scanner(pattern).run();
	} catch (e) {
		if (e instanceof Fail) return e.code;
		throw e;
	}
	try {
		return new RegExp(pattern, caseInsensitive ? 'ui' : 'u');
	} catch {
		return INVALID;
	}
}

/** Checks a pattern against the subset; `null` when it is accepted. */
export function checkPattern(pattern: string): PatternCode | null {
	const r = compilePattern(pattern);
	return r instanceof RegExp ? null : r;
}
