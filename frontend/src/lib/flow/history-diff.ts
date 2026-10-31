// Semantic diff for one history entry (`versions/v0.5-collaboration.md` "Web 交互": "history 面板按
// actor/message/time 显示 semantic diff，不暴露 peer id 或二进制 update").
//
// The diff is read through `CommandService.diff(from, to)` (`GET .../diff`). Its `semantic_diff`
// is `{title:{before,after}|null, nodes:{added,removed,changed}}` (`apps/api/src/flow/query.rs::
// get_object_diff`, `crates/collab-core/src/semantic.rs::SemanticDiff`). This module turns it into
// display lines that carry node kind, text and property changes only: node ids, frontiers and
// anything byte-shaped are never copied into a line.

import type { FlowDiffResponse } from '#lib/api/flow.js';
import { FlowCommandService } from './command-service';
import { flowErrorI18nKey } from './errors';
import type { FlowError } from './types';

export type DiffOperation = 'title' | 'added' | 'removed' | 'changed' | 'moved';

export interface DiffPropertyChange {
	readonly key: string;
	readonly before: string | null;
	readonly after: string | null;
}

export interface DiffLine {
	readonly op: DiffOperation;
	/** `NodeKind` wire value (`block`, `collection_field`, ...), `null` for the title line. */
	readonly kind: string | null;
	readonly before: string | null;
	readonly after: string | null;
	readonly properties: readonly DiffPropertyChange[];
}

export const DIFF_OPERATION_KEYS: Readonly<Record<DiffOperation, string>> = {
	title: 'flow.history.diff.op.title',
	added: 'flow.history.diff.op.added',
	removed: 'flow.history.diff.op.removed',
	changed: 'flow.history.diff.op.changed',
	moved: 'flow.history.diff.op.moved'
};

export const DIFF_KIND_KEYS: Readonly<Record<string, string>> = {
	block: 'flow.history.diff.kind.block',
	collection_field: 'flow.history.diff.kind.collection_field',
	collection_view: 'flow.history.diff.kind.collection_view',
	record_property: 'flow.history.diff.kind.record_property',
	navigator_node: 'flow.history.diff.kind.navigator_node'
};

export function diffKindKey(kind: string | null): string {
	return (kind && DIFF_KIND_KEYS[kind]) || 'flow.history.diff.kind.other';
}

interface WireNode {
	kind?: unknown;
	text?: unknown;
	properties?: unknown;
	parent?: unknown;
	order_key?: unknown;
	deleted?: unknown;
}

function asRecord(value: unknown): Record<string, unknown> | null {
	return typeof value === 'object' && value !== null && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: null;
}

function text(node: WireNode | null): string | null {
	return node && typeof node.text === 'string' ? node.text : null;
}

function kind(node: WireNode | null): string | null {
	return node && typeof node.kind === 'string' ? node.kind : null;
}

function propertyChanges(before: WireNode, after: WireNode): DiffPropertyChange[] {
	const a = asRecord(before.properties) ?? {};
	const b = asRecord(after.properties) ?? {};
	const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])].sort();
	const out: DiffPropertyChange[] = [];
	for (const key of keys) {
		const was = typeof a[key] === 'string' ? (a[key] as string) : null;
		const now = typeof b[key] === 'string' ? (b[key] as string) : null;
		if (was !== now) out.push({ key, before: was, after: now });
	}
	return out;
}

/** Display lines for a `semantic_diff`, or `null` when the value is not the documented shape. */
export function semanticDiffLines(semanticDiff: unknown): DiffLine[] | null {
	const root = asRecord(semanticDiff);
	if (!root) return null;
	const lines: DiffLine[] = [];
	const title = asRecord(root.title);
	if (title && (typeof title.before === 'string' || typeof title.after === 'string')) {
		lines.push({
			op: 'title',
			kind: null,
			before: typeof title.before === 'string' ? title.before : null,
			after: typeof title.after === 'string' ? title.after : null,
			properties: []
		});
	}
	const nodes = asRecord(root.nodes);
	if (!nodes) return root.title === undefined ? null : lines;
	for (const value of Object.values(asRecord(nodes.added) ?? {})) {
		const node = asRecord(value) as WireNode | null;
		lines.push({ op: 'added', kind: kind(node), before: null, after: text(node), properties: [] });
	}
	for (const value of Object.values(asRecord(nodes.removed) ?? {})) {
		const node = asRecord(value) as WireNode | null;
		lines.push({
			op: 'removed',
			kind: kind(node),
			before: text(node),
			after: null,
			properties: []
		});
	}
	for (const value of Object.values(asRecord(nodes.changed) ?? {})) {
		const change = asRecord(value);
		const before = (asRecord(change?.before) ?? {}) as WireNode;
		const after = (asRecord(change?.after) ?? {}) as WireNode;
		const properties = propertyChanges(before, after);
		const sameText = text(before) === text(after);
		lines.push({
			op:
				after.deleted === true
					? 'removed'
					: sameText && properties.length === 0
						? 'moved'
						: 'changed',
			kind: kind(after) ?? kind(before),
			before: text(before),
			after: text(after),
			properties
		});
	}
	return lines;
}

export type HistoryDiffResult =
	| { readonly status: 'ready'; readonly lines: readonly DiffLine[] }
	| { readonly status: 'failed'; readonly messageKey: string };

/** The `(from, to)` pair for the change recorded at history `seq`. */
export function diffRangeFor(seq: number): { from: number; to: number } {
	return { from: Math.max(0, seq - 1), to: seq };
}

function isFlowError(value: unknown): value is FlowError {
	return (
		typeof value === 'object' &&
		value !== null &&
		typeof (value as { code?: unknown }).code === 'string'
	);
}

/** Loads (and caches per seq) the semantic diff of one history entry. */
export class HistoryDiffLoader {
	private readonly cache = new Map<number, HistoryDiffResult>();

	constructor(
		private readonly objectId: string,
		private readonly commands: Pick<FlowCommandService, 'diff'> = new FlowCommandService()
	) {}

	async load(seq: number): Promise<HistoryDiffResult> {
		const cached = this.cache.get(seq);
		if (cached) return cached;
		const { from, to } = diffRangeFor(seq);
		let response: FlowDiffResponse;
		try {
			response = await this.commands.diff(this.objectId, from, to);
		} catch (error) {
			// Not cached: a failed load can be retried by expanding again.
			return {
				status: 'failed',
				messageKey: isFlowError(error) ? flowErrorI18nKey(error) : 'flow.history.diff.error'
			};
		}
		const lines = semanticDiffLines(response.semantic_diff);
		const result: HistoryDiffResult = lines
			? { status: 'ready', lines }
			: { status: 'failed', messageKey: 'flow.history.diff.error' };
		this.cache.set(seq, result);
		return result;
	}
}
