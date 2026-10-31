// Standalone Collection creation from the navigator (`ui-surface-v1.md` "后续版本 UI 派生" v0.6:
// "Navigator 的 standalone Collection 创建走通用 object create"; `rest-api-v1.md` v0.6:
// `POST .../flow/objects` with `object_type=collection`, `initial_fields[]`, `initial_view`).
//
// Rules this module owns:
// - one intent = one idempotency key AND one set of field/view ids. The server keys replay on the
//   whole create body including the schema (`flow/command.rs::CreateObjectIdempotencyBody`), so
//   a retry of the same intent must resend the identical body; editing the form makes it a new
//   intent with a new key and new ids;
// - the request goes through `CommandService.createObject`, never a client-side
//   "create object then add fields" sequence;
// - errors are classified by `error_code` only; `server_rejected` is permanent (no retry offered).

import type {
	AcceptedChange,
	CreateFlowObjectInput,
	FlowCollectionInitialField,
	FlowCollectionInitialView
} from '#lib/api/flow.js';
import { flowErrorI18nKey } from './errors';
import type { FlowError } from './types';

export type CollectionViewType = 'table' | 'board';
export const COLLECTION_VIEW_TYPES = [
	'table',
	'board'
] as const satisfies readonly CollectionViewType[];
export const COLLECTION_VIEW_NAME_KEYS: Readonly<Record<CollectionViewType, string>> = {
	table: 'flow.nav.createCollection.viewName.table',
	board: 'flow.nav.createCollection.viewName.board'
};
/** `flow/command.rs::TITLE_MAX_CHARS`; field labels and the view name share
 * `flow/collections.rs::LABEL_MAX_CHARS` (also 500). */
export const COLLECTION_TITLE_MAX_CHARS = 500;

export interface CollectionDraft {
	readonly title: string;
	/** Labels of the initial `text` fields, in order. Empty is allowed. */
	readonly fieldLabels: readonly string[];
	readonly viewType: CollectionViewType;
	/** Stored name of the initial view (localized by the caller). */
	readonly viewName: string;
}

export interface CollectionCreator {
	createObject(
		workspaceId: string,
		input: Omit<CreateFlowObjectInput, 'idempotency_key'>,
		options?: { idempotencyKey?: string }
	): Promise<AcceptedChange>;
}

export type CollectionValidation =
	| 'ok'
	| 'title_required'
	| 'title_too_long'
	| 'field_label_required';

export type CreateCollectionOutcome =
	| { readonly kind: 'created'; readonly change: AcceptedChange }
	| { readonly kind: 'invalid'; readonly reason: Exclude<CollectionValidation, 'ok'> }
	| {
			readonly kind: 'failed';
			readonly key: string;
			readonly code: string;
			readonly retryable: boolean;
	  };

export function validateCollectionDraft(draft: CollectionDraft): CollectionValidation {
	const title = draft.title.trim();
	if (title.length === 0) return 'title_required';
	if ([...title].length > COLLECTION_TITLE_MAX_CHARS) return 'title_too_long';
	if (draft.fieldLabels.some((label) => label.trim().length === 0)) return 'field_label_required';
	if (draft.fieldLabels.some((label) => [...label.trim()].length > COLLECTION_TITLE_MAX_CHARS))
		return 'title_too_long';
	return 'ok';
}

function newId(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: `id-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

function fingerprint(draft: CollectionDraft): string {
	return JSON.stringify([
		draft.title.trim(),
		draft.fieldLabels.map((label) => label.trim()),
		draft.viewType,
		draft.viewName.trim()
	]);
}

interface Intent {
	readonly fingerprint: string;
	readonly idempotencyKey: string;
	readonly fieldIds: readonly string[];
	readonly viewId: string;
}

export class CreateCollectionSession {
	private intent: Intent | null = null;
	private submitting = false;

	constructor(
		private readonly workspaceId: string,
		private readonly commands: CollectionCreator,
		private readonly mintId: () => string = newId
	) {}

	get busy(): boolean {
		return this.submitting;
	}

	/** The idempotency key the next submit of `draft` would use (same draft -> same key). */
	keyFor(draft: CollectionDraft): string {
		return this.intentFor(draft).idempotencyKey;
	}

	/** The exact `POST .../flow/objects` body (minus the key) for `draft`. */
	requestFor(draft: CollectionDraft): Omit<CreateFlowObjectInput, 'idempotency_key'> {
		const intent = this.intentFor(draft);
		const initial_fields: FlowCollectionInitialField[] = draft.fieldLabels.map((label, index) => ({
			field_id: intent.fieldIds[index],
			label: label.trim(),
			field_type: 'text',
			config: {},
			index
		}));
		const initial_view: FlowCollectionInitialView = {
			view_id: intent.viewId,
			name: draft.viewName.trim(),
			view_type: draft.viewType,
			config: {},
			index: 0
		};
		return {
			object_type: 'collection',
			title: draft.title.trim(),
			initial_fields,
			initial_view
		};
	}

	async submit(draft: CollectionDraft): Promise<CreateCollectionOutcome> {
		const validation = validateCollectionDraft(draft);
		if (validation !== 'ok') return { kind: 'invalid', reason: validation };
		if (this.submitting)
			return {
				kind: 'failed',
				key: 'flow.nav.createCollection.busy',
				code: 'busy',
				retryable: true
			};
		const body = this.requestFor(draft);
		const idempotencyKey = this.keyFor(draft);
		this.submitting = true;
		try {
			const change = await this.commands.createObject(this.workspaceId, body, { idempotencyKey });
			this.intent = null;
			return { kind: 'created', change };
		} catch (raw) {
			const error = raw as FlowError;
			const code = typeof error?.code === 'string' ? error.code : 'invalid_update';
			return {
				kind: 'failed',
				key:
					typeof error?.code === 'string'
						? flowErrorI18nKey(error)
						: 'flow.nav.createCollection.failed',
				code,
				retryable: code !== 'server_rejected' && error?.recoverable === true
			};
		} finally {
			this.submitting = false;
		}
	}

	private intentFor(draft: CollectionDraft): Intent {
		const print = fingerprint(draft);
		if (this.intent && this.intent.fingerprint === print) return this.intent;
		this.intent = {
			fingerprint: print,
			idempotencyKey: this.mintId(),
			fieldIds: draft.fieldLabels.map(() => this.mintId()),
			viewId: this.mintId()
		};
		return this.intent;
	}
}
