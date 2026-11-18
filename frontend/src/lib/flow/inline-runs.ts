/** Safe public read rendering of the canonical UTF-8 byte-range mark contract. */
import { safeEditorHref, type FlowMark } from './editor-schema';
export interface PageInlineSpan {
	start: number;
	end: number;
	mark: FlowMark;
	href?: string;
}
export interface PageInlineRun {
	text: string;
	marks: { mark: FlowMark; href?: string }[];
}
const names: FlowMark[] = ['bold', 'italic', 'strike', 'code', 'link'];
export function pageInlineRuns(
	text: string,
	spans: readonly PageInlineSpan[] = []
): PageInlineRun[] {
	if (!spans.length) return text ? [{ text, marks: [] }] : [];
	const bytesToChars = new Map<number, number>([[0, 0]]);
	let bytes = 0,
		chars = 0;
	const encoder = new TextEncoder();
	for (const char of text) {
		bytes += encoder.encode(char).length;
		chars += char.length;
		bytesToChars.set(bytes, chars);
	}
	const valid = spans.filter(
		(span) =>
			names.includes(span.mark) &&
			Number.isInteger(span.start) &&
			Number.isInteger(span.end) &&
			span.start < span.end &&
			bytesToChars.has(span.start) &&
			bytesToChars.has(span.end) &&
			(span.mark !== 'link' || (typeof span.href === 'string' && safeEditorHref(span.href)))
	);
	const boundaries = [
		...new Set([0, bytes, ...valid.flatMap((span) => [span.start, span.end])])
	].sort((a, b) => a - b);
	const runs: PageInlineRun[] = [];
	for (let i = 0; i < boundaries.length - 1; i++) {
		const start = boundaries[i],
			end = boundaries[i + 1];
		const marks = valid
			.filter((span) => span.start <= start && span.end >= end)
			.sort((a, b) => names.indexOf(a.mark) - names.indexOf(b.mark))
			.map((span) => ({ mark: span.mark, ...(span.mark === 'link' ? { href: span.href } : {}) }));
		const unique = marks.filter(
			(mark, index) => marks.findIndex((candidate) => candidate.mark === mark.mark) === index
		);
		runs.push({
			text: text.slice(bytesToChars.get(start)!, bytesToChars.get(end)!),
			marks: unique
		});
	}
	return runs;
}

/** Viewer-only folding and sibling numbering; the server supplies active Tree preorder only. */
export function pageVisibleBlocks<T extends { kind: string; indent: number; order?: number }>(
	blocks: readonly T[],
	collapsed: ReadonlySet<number>
): { block: T; index: number; order: number }[] {
	let hiddenDepth: number | null = null;
	const numbers = new Map<number, number>();
	return blocks.flatMap((block, index) => {
		const depth = Math.max(0, Math.min(7, block.indent));
		for (const key of numbers.keys()) if (key > depth) numbers.delete(key);
		let order = block.order ?? 1;
		if (block.kind === 'orderedItem') {
			order = (numbers.get(depth) ?? order - 1) + 1;
			numbers.set(depth, order);
		} else numbers.delete(depth);
		if (hiddenDepth !== null && depth <= hiddenDepth) hiddenDepth = null;
		if (hiddenDepth !== null) return [];
		if (block.kind === 'toggle' && collapsed.has(index)) hiddenDepth = depth;
		return [{ block, index, order }];
	});
}
