// The recovery draft as the user takes it away (`ui-surface-v1.md` step 5: "不能重放的 intent 进入
// recovery draft，并提供下载/复制动作"). Two renderings of the same entries: a JSON document for
// download (every refused intent with its reason and the raw update bytes, so nothing is lost), and
// a plain-text summary for the clipboard. Labels come from the caller's translator, so this module
// carries no user-facing copy of its own.

import type { RecoveryDraftEntry, RecoveryDraftReason } from './outbox';
import { flowErrorDetailKey } from './errors';
import type { FlowErrorCode } from './types';

export const RECOVERY_DRAFT_SCHEMA = 'sylvode.flow.recovery-draft.v1';

/** The label of each draft reason source (an exhaustive map, so the i18n gate can check it). */
export const RECOVERY_SOURCE_KEYS: Record<RecoveryDraftEntry['reason']['source'], string> = {
	server: 'flow.recovery.source.server',
	precheck: 'flow.recovery.source.precheck',
	dependency: 'flow.recovery.source.dependency'
};

function toBase64(bytes: Uint8Array): string {
	let binary = '';
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

/** The local content at the time of the export: the engine doc's JSON value (what the canvas was
 * showing, including the refused edits if the doc has not been rebuilt yet). */
export interface RecoveryDraftDocument {
	readonly schema: typeof RECOVERY_DRAFT_SCHEMA;
	readonly object_id: string;
	readonly document_id: string;
	readonly exported_at: string;
	readonly entries: ReadonlyArray<{
		readonly update_id: string;
		readonly idempotency_key: string;
		readonly intent: unknown;
		readonly base_frontier: string;
		readonly bytes_base64: string;
		readonly enqueued_at: string;
		readonly rejected_at: string;
		readonly reason: {
			readonly code: string;
			readonly source: RecoveryDraftReason['source'];
			readonly details?: unknown;
			readonly root_update_id?: string;
		};
	}>;
	readonly local_text: string;
	readonly local_document: unknown;
}

export function buildRecoveryDraftDocument(input: {
	objectId: string;
	documentId: string;
	entries: ReadonlyArray<RecoveryDraftEntry>;
	localDocument: unknown;
	now?: number;
}): RecoveryDraftDocument {
	return {
		schema: RECOVERY_DRAFT_SCHEMA,
		object_id: input.objectId,
		document_id: input.documentId,
		exported_at: new Date(input.now ?? Date.now()).toISOString(),
		entries: input.entries.map((entry) => ({
			update_id: entry.updateId,
			idempotency_key: entry.idempotencyKey,
			intent: entry.intent,
			base_frontier: entry.baseFrontier,
			bytes_base64: toBase64(entry.bytes),
			enqueued_at: new Date(entry.enqueuedAt).toISOString(),
			rejected_at: new Date(entry.rejectedAt).toISOString(),
			reason: {
				code: entry.reason.code,
				source: entry.reason.source,
				...(entry.reason.details === undefined ? {} : { details: entry.reason.details }),
				...(entry.reason.rootUpdateId ? { root_update_id: entry.reason.rootUpdateId } : {})
			}
		})),
		local_text: plainTextOf(input.localDocument),
		local_document: input.localDocument
	};
}

/**
 * The visible text of a `loro-prosemirror` doc value (`{doc:{nodeName, attributes, children:[...]}}`
 * where text leaves are strings), one block per line. Anything else contributes nothing.
 */
export function plainTextOf(value: unknown): string {
	const lines: string[] = [];
	const walk = (node: unknown): string => {
		if (typeof node === 'string') return node;
		if (typeof node !== 'object' || node === null) return '';
		const children = (node as { children?: unknown }).children;
		if (!Array.isArray(children)) return '';
		const parts = children.map(walk);
		const hasBlockChildren = children.some((child) => typeof child === 'object' && child !== null);
		if (hasBlockChildren) {
			for (const part of parts) if (part !== '') lines.push(part);
			return '';
		}
		return parts.join('');
	};
	const root =
		typeof value === 'object' && value !== null ? (value as { doc?: unknown }).doc : null;
	const tail = walk(root ?? value);
	if (tail !== '') lines.push(tail);
	return lines.join('\n');
}

/** The clipboard text: a header, one line per refused intent with its reason, then the local text. */
export function recoveryDraftText(
	draft: RecoveryDraftDocument,
	translate: (key: string, values?: Record<string, string | number>) => string
): string {
	const lines = [translate('flow.recovery.textHeader', { count: draft.entries.length })];
	draft.entries.forEach((entry, index) => {
		const intent = entry.intent as { kind?: unknown } | null;
		const kind = typeof intent?.kind === 'string' ? intent.kind : 'content_edit';
		lines.push(
			translate('flow.recovery.textLine', {
				index: index + 1,
				intent: translate(
					kind === 'navigator_reorder'
						? 'flow.recovery.intent.navigator_reorder'
						: 'flow.recovery.intent.content_edit'
				),
				reason: translate(
					flowErrorDetailKey({
						code: entry.reason.code as FlowErrorCode,
						details: entry.reason.details
					}),
					safeValues(entry.reason.details)
				),
				source: translate(RECOVERY_SOURCE_KEYS[entry.reason.source])
			})
		);
	});
	if (draft.local_text !== '') {
		lines.push('', translate('flow.recovery.textContent'), draft.local_text);
	}
	return lines.join('\n');
}

function safeValues(details: unknown): Record<string, string | number> {
	if (typeof details !== 'object' || details === null) return {};
	const out: Record<string, string | number> = {};
	for (const [key, value] of Object.entries(details as Record<string, unknown>)) {
		if (typeof value === 'string' || typeof value === 'number') out[key] = value;
	}
	return out;
}
