// Pure editing rules for the Flow page canvas: Markdown triggers, inline-mark addresses, the
// block clipboard, and multi-block moves. The ProseMirror adapter applies these; it does not
// decide them. Shortcuts are refused while an IME composition is open so an intermediate pinyin
// string is never treated as a trigger.

export type FlowMarkName = 'bold' | 'italic' | 'code' | 'strike' | 'link';

export type FlowBlockKind = 'paragraph' | 'heading' | 'bulletItem' | 'codeBlock';

export interface BlockShortcut {
	readonly type: FlowBlockKind;
	readonly level?: 1 | 2 | 3;
	/** Characters at the end of the text already in the block that belong to the trigger. */
	readonly deleteBefore: number;
	/** The part of the just-typed string that is not the trigger and should be inserted. */
	readonly remainder: string;
}

const BLOCK_TRIGGERS: ReadonlyArray<{
	readonly pattern: string;
	readonly type: FlowBlockKind;
	readonly level?: 1 | 2 | 3;
}> = [
	{ pattern: '### ', type: 'heading', level: 3 },
	{ pattern: '## ', type: 'heading', level: 2 },
	{ pattern: '# ', type: 'heading', level: 1 },
	{ pattern: '- ', type: 'bulletItem' },
	{ pattern: '* ', type: 'bulletItem' },
	{ pattern: '+ ', type: 'bulletItem' }
];

/** True when a keystroke or text input must not change block type or marks. */
export function acceptEditorInput(input: { composing: boolean; isComposing: boolean }): boolean {
	return !input.composing && !input.isComposing;
}

/**
 * A block trigger at the start of a block, completed by this insertion.
 * `before` is the block text before the caret; `inserted` is the new text; `after` is the
 * block text after the caret. `# ` / `## ` / `### ` / `- ` / `* ` / `+ ` convert the block and
 * keep whatever was typed after the trigger. A fence of exactly three backticks, with nothing
 * else in the block, becomes a code block.
 */
export function matchBlockShortcut(
	before: string,
	inserted: string,
	after: string
): BlockShortcut | null {
	if (after === '' && before + inserted === '```') {
		return { type: 'codeBlock', deleteBefore: before.length, remainder: '' };
	}
	const combined = before + inserted;
	for (const trigger of BLOCK_TRIGGERS) {
		if (!combined.startsWith(trigger.pattern)) continue;
		if (before.length >= trigger.pattern.length) continue;
		if (!trigger.pattern.startsWith(before)) continue;
		const consumed = trigger.pattern.length - before.length;
		return {
			type: trigger.type,
			level: trigger.level,
			deleteBefore: before.length,
			remainder: inserted.slice(consumed)
		};
	}
	return null;
}

export interface InlineWrap {
	readonly mark: Exclude<FlowMarkName, 'link'>;
	/** Index in `before` where the opening delimiter starts. */
	readonly openStart: number;
	readonly openLength: number;
	readonly contentStart: number;
	readonly contentEnd: number;
	/** Characters at the end of `before` that are a partial closing delimiter. */
	readonly deleteTail: number;
}

/**
 * Closing `**` / `*` / `` ` `` / `~~` around a non-empty run in `before`, completed by
 * `inserted`. The caller deletes the delimiters and applies the mark, which is how the
 * characters disappear the way they do in Notion.
 */
export function matchInlineWrap(before: string, inserted: string): InlineWrap | null {
	return (
		matchPair(before, inserted, '**', 'bold') ??
		matchSingle(before, inserted, '*', 'italic') ??
		matchPair(before, inserted, '~~', 'strike') ??
		matchSingle(before, inserted, '`', 'code')
	);
}

function matchPair(
	before: string,
	inserted: string,
	delimiter: string,
	mark: InlineWrap['mark']
): InlineWrap | null {
	if (inserted !== delimiter.slice(-1) || !before.endsWith(delimiter.slice(0, -1))) return null;
	if (before.endsWith(delimiter)) return null;
	const upto = before.slice(0, before.length - (delimiter.length - 1));
	const open = upto.lastIndexOf(delimiter);
	if (open < 0) return null;
	const contentStart = open + delimiter.length;
	const content = upto.slice(contentStart);
	if (!plainContent(content)) return null;
	return {
		mark,
		openStart: open,
		openLength: delimiter.length,
		contentStart,
		contentEnd: upto.length,
		deleteTail: delimiter.length - 1
	};
}

function matchSingle(
	before: string,
	inserted: string,
	delimiter: string,
	mark: InlineWrap['mark']
): InlineWrap | null {
	if (inserted !== delimiter || before.endsWith(delimiter)) return null;
	const open = before.lastIndexOf(delimiter);
	if (open < 0) return null;
	if (open > 0 && before[open - 1] === delimiter) return null;
	const content = before.slice(open + delimiter.length);
	if (!plainContent(content)) return null;
	return {
		mark,
		openStart: open,
		openLength: delimiter.length,
		contentStart: open + delimiter.length,
		contentEnd: before.length,
		deleteTail: 0
	};
}

function plainContent(content: string): boolean {
	return content.length > 0 && !content.includes('\n') && !content.includes('*') && !content.includes('`') && !content.includes('~');
}

/** Where a removed half-open block range `[start, end)` should be reinserted. `dest` is the
 * index to insert before in the original list. A destination inside the range is not a move. */
export function insertIndexAfterRemoval(start: number, end: number, dest: number): number | null {
	if (!Number.isInteger(start) || !Number.isInteger(end) || !Number.isInteger(dest)) return null;
	if (start < 0 || end <= start || dest < 0) return null;
	if (dest > start && dest < end) return null;
	if (dest >= end) return dest - (end - start);
	return dest;
}

export interface ClipboardMark {
	readonly start: number;
	readonly end: number;
	readonly type: FlowMarkName;
	readonly href?: string;
}

export interface ClipboardBlock {
	readonly type: FlowBlockKind;
	readonly indent: number;
	readonly level?: number;
	readonly text: string;
	readonly marks: readonly ClipboardMark[];
}

export const FLOW_BLOCK_CLIPBOARD = 'application/x-sylvode-flow-blocks';

const BLOCK_KINDS: ReadonlySet<string> = new Set(['paragraph', 'heading', 'bulletItem', 'codeBlock']);
const MARK_NAMES: ReadonlySet<string> = new Set(['bold', 'italic', 'code', 'strike', 'link']);

export function encodeBlocks(blocks: readonly ClipboardBlock[]): string {
	return JSON.stringify({ schema: 'sylvode.flow.blocks.v1', blocks });
}

/** `null` when the payload is not this editor's block clipboard. */
export function decodeBlocks(raw: string): ClipboardBlock[] | null {
	let parsed: unknown;
	try {
		parsed = JSON.parse(raw);
	} catch {
		return null;
	}
	if (typeof parsed !== 'object' || parsed === null) return null;
	const root = parsed as { schema?: unknown; blocks?: unknown };
	if (root.schema !== 'sylvode.flow.blocks.v1' || !Array.isArray(root.blocks)) return null;
	const blocks: ClipboardBlock[] = [];
	for (const entry of root.blocks) {
		const block = decodeBlock(entry);
		if (!block) return null;
		blocks.push(block);
	}
	return blocks.length > 0 ? blocks : null;
}

function decodeBlock(entry: unknown): ClipboardBlock | null {
	if (typeof entry !== 'object' || entry === null) return null;
	const record = entry as Record<string, unknown>;
	if (typeof record.type !== 'string' || !BLOCK_KINDS.has(record.type)) return null;
	if (typeof record.text !== 'string') return null;
	if (typeof record.indent !== 'number' || !Number.isInteger(record.indent) || record.indent < 0 || record.indent > 7)
		return null;
	let level: number | undefined;
	if (record.type === 'heading') {
		if (record.level !== 1 && record.level !== 2 && record.level !== 3) return null;
		level = record.level;
	}
	if (!Array.isArray(record.marks)) return null;
	const marks: ClipboardMark[] = [];
	for (const mark of record.marks) {
		const decoded = decodeMark(mark, record.text);
		if (!decoded) return null;
		marks.push(decoded);
	}
	return {
		type: record.type as FlowBlockKind,
		indent: record.indent,
		...(level === undefined ? {} : { level: level as 1 | 2 | 3 }),
		text: record.text,
		marks
	};
}

function decodeMark(entry: unknown, text: string): ClipboardMark | null {
	if (typeof entry !== 'object' || entry === null) return null;
	const record = entry as Record<string, unknown>;
	if (typeof record.type !== 'string' || !MARK_NAMES.has(record.type)) return null;
	if (typeof record.start !== 'number' || typeof record.end !== 'number') return null;
	if (!Number.isInteger(record.start) || !Number.isInteger(record.end)) return null;
	if (record.start < 0 || record.end <= record.start || record.end > text.length) return null;
	if (record.type === 'link') {
		if (typeof record.href !== 'string' || !safeHref(record.href)) return null;
		return { start: record.start, end: record.end, type: 'link', href: record.href };
	}
	return { start: record.start, end: record.end, type: record.type as FlowMarkName };
}

/** Addresses the link mark and the link popover will accept. */
export function safeHref(href: string): boolean {
	if (href.length === 0 || href.length > 2048) return false;
	if (/[\s\u0000-\u001f\u007f]/.test(href)) return false;
	const lower = href.toLowerCase();
	if (lower.startsWith('javascript:') || lower.startsWith('data:') || lower.startsWith('vbscript:'))
		return false;
	return (
		lower.startsWith('https://') ||
		lower.startsWith('http://') ||
		lower.startsWith('mailto:') ||
		(href.startsWith('/') && !href.startsWith('//')) ||
		href.startsWith('#')
	);
}
