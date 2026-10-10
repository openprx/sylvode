// Page slash-menu insertions that reach beyond plain text: the Collection embed
// (`ui-surface-v1.md` "后续版本 UI 派生" v0.6) and the Forms Bridge Reference / Embed / Convert
// entries (`versions/v0.7-forms-bridge.md` "Web 交互").
//
// Rules this module owns:
// - Collection: ONE `CommandService.execute(create_collection_embed)` per intent. The server
//   creates the Collection, its document, the Page's embed block and the `embeds` relation in one
//   transaction (`flow/collections.rs::create_collection_embed`); the client never chains
//   "create object, then insert block". While the command is in flight a single provisional block
//   is shown. It is local and ephemeral (never written to the CRDT); a failure removes it whole,
//   and "retry" resends the same idempotency key and the same body (the field and view ids are
//   part of the server's replay body, `EmbedIdempotencyBody`).
// - Reference / Embed: `POST .../references` with `display.mode` `reference` or `embed`. The
//   bridge row (`flow_bridge_references`) is the source of truth; the Page only carries a
//   lightweight placeholder block that points at the `reference_id` (the backend has no
//   reference block type), so no Forms content is ever copied into the Page document. Removal is
//   confirmed first and only calls `DELETE .../references/{id}`; the target is never touched.
// - An Embed renders target content only when the request-time `permission_state` allows
//   `form.view`; otherwise it is a placeholder and no Forms data is requested at all.
// - Errors are classified by `error_code` only (`errors.ts`), never by `message`.

import type { ApiResult, PaginatedData } from '$lib/api/client';
import type {
	AcceptedChange,
	BridgePermissionState,
	CollectionEmbedCommandResult,
	CreateCollectionEmbedPayload,
	FlowCommandInput,
	FlowReferenceItem,
	FlowReferenceList,
	FlowReferenceReceipt
} from '$lib/api/flow';
import { formsApi, type FormField, type FormRecord, type UniversalForm } from '$lib/api/forms';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';
import { listWorkspaceProjects } from './package-export';
import type { FlowError } from './types';

// ---- slash menu catalogue ---------------------------------------------------------------------

export type SlashBridgeAction = 'collection' | 'reference' | 'embed' | 'convert';

export interface SlashBridgeOption {
	readonly action: SlashBridgeAction;
	readonly labelKey: string;
	/** One line that says who owns the result (`v0.7-forms-bridge.md`: "解释所有权结果"). */
	readonly noteKey: string;
}

export const SLASH_BRIDGE_OPTIONS: readonly SlashBridgeOption[] = [
	{
		action: 'collection',
		labelKey: 'flow.canvas.slashMenu.collection',
		noteKey: 'flow.canvas.slashMenu.collectionNote'
	},
	{
		action: 'reference',
		labelKey: 'flow.canvas.slashMenu.referenceForms',
		noteKey: 'flow.canvas.slashMenu.referenceNote'
	},
	{
		action: 'embed',
		labelKey: 'flow.canvas.slashMenu.embedForms',
		noteKey: 'flow.canvas.slashMenu.embedNote'
	},
	{
		action: 'convert',
		labelKey: 'flow.canvas.slashMenu.convertForms',
		noteKey: 'flow.canvas.slashMenu.convertNote'
	}
];

/** The N4 conversion wizard for this Page. */
export function convertWizardPath(workspaceId: string, objectId: string): string {
	return `/workspace/${workspaceId}/flow/${objectId}/convert`;
}

export function newIdempotencyKey(): string {
	return typeof crypto !== 'undefined' && 'randomUUID' in crypto
		? crypto.randomUUID()
		: `idem-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

/** Failure shown to the user. `retryable` is false for permanent failures (`server_rejected`
 * and every non-recoverable typed code), which must not offer a retry. */
export interface SlashFailure {
	readonly key: string;
	readonly code: string;
	readonly retryable: boolean;
}

/** Classifies a thrown `CommandService` error. A typed server error keeps its own key and its
 * `recoverable` flag; anything else (a transport failure or a legacy envelope with no
 * `error_code`) is reported with the caller's generic key and stays retryable, which is safe
 * because a retry resends the same key. */
function failureFrom(raw: unknown, genericKey: string): SlashFailure {
	const error = raw as FlowError | null;
	if (error && typeof error.code === 'string' && error.origin === 'server') {
		return {
			key: flowErrorI18nKey(error),
			code: error.code,
			retryable: error.code !== 'server_rejected' && error.recoverable === true
		};
	}
	return { key: genericKey, code: 'transport', retryable: true };
}

/** Same classification for a raw `ApiResult` envelope (the bridge endpoints return one). */
function failureFromEnvelope(
	result: { code: number; error_code?: string | null; details?: unknown },
	genericKey: string
): SlashFailure {
	const error = flowErrorFromEnvelope(result);
	return failureFrom(error, genericKey);
}

// ---- Collection embed -------------------------------------------------------------------------

export const COLLECTION_EMBED_TITLE_MAX_CHARS = 500;
export const COLLECTION_EMBED_FAILED_KEY = 'flow.canvas.slashMenu.collectionFailed';

export interface EmbedCommands {
	execute(
		objectId: string,
		command: FlowCommandInput,
		options: { idempotencyKey: string }
	): Promise<AcceptedChange>;
}

/** The single provisional block. `pending` while the command is in flight; `accepted` once the
 * server answered, until the real embed block shows up in the Page document. */
export interface ProvisionalEmbed {
	readonly localId: string;
	readonly title: string;
	readonly status: 'pending' | 'accepted';
	readonly collectionId: string | null;
	readonly blockId: string | null;
}

export interface CollectionEmbedSnapshot {
	readonly provisional: ProvisionalEmbed | null;
	readonly failure: SlashFailure | null;
	readonly busy: boolean;
	/** The title of the failed intent, so the error can say what failed. */
	readonly failedTitle: string | null;
}

export type CollectionEmbedOutcome =
	| {
			readonly kind: 'created';
			readonly collectionId: string | null;
			readonly blockId: string | null;
	  }
	| { readonly kind: 'invalid'; readonly reason: 'title_required' | 'title_too_long' }
	| { readonly kind: 'failed'; readonly failure: SlashFailure }
	| { readonly kind: 'busy' };

/** Localised labels the new Collection starts with (one text field, one table view). */
export interface CollectionEmbedLabels {
	readonly fieldLabel: string;
	readonly viewName: string;
}

interface EmbedIntent {
	readonly idempotencyKey: string;
	readonly title: string;
	readonly payload: CreateCollectionEmbedPayload;
}

export function validateEmbedTitle(title: string): 'ok' | 'title_required' | 'title_too_long' {
	const trimmed = title.trim();
	if (trimmed.length === 0) return 'title_required';
	if ([...trimmed].length > COLLECTION_EMBED_TITLE_MAX_CHARS) return 'title_too_long';
	return 'ok';
}

/** Reads the created ids from `command_result`. The server writes `collection_id` / `block_id`
 * (`flow/collections.rs::create_collection_embed`); `rest-api-v1.md` v0.6 names them
 * `created_collection_id` / `embed_block_id`, so both spellings are read. */
export function embedResultIds(change: AcceptedChange): {
	collectionId: string | null;
	blockId: string | null;
} {
	const raw = (change.command_result ?? null) as
		| (Partial<CollectionEmbedCommandResult> & {
				created_collection_id?: unknown;
				embed_block_id?: unknown;
		  })
		| null;
	const pick = (...values: unknown[]): string | null => {
		for (const value of values) if (typeof value === 'string' && value.length > 0) return value;
		return null;
	};
	return {
		collectionId: pick(raw?.collection_id, raw?.created_collection_id),
		blockId: pick(raw?.block_id, raw?.embed_block_id)
	};
}

export class CollectionEmbedSession {
	private intent: EmbedIntent | null = null;
	private provisional: ProvisionalEmbed | null = null;
	private failure: SlashFailure | null = null;
	private busy = false;

	constructor(
		private readonly pageId: string,
		private readonly commands: EmbedCommands,
		private readonly onChange: (snapshot: CollectionEmbedSnapshot) => void = () => {},
		private readonly mintId: () => string = newIdempotencyKey
	) {}

	snapshot(): CollectionEmbedSnapshot {
		return {
			provisional: this.provisional,
			failure: this.failure,
			busy: this.busy,
			failedTitle: this.failure ? (this.intent?.title ?? null) : null
		};
	}

	/** The key the pending/failed intent uses (`null` when there is none). */
	get idempotencyKey(): string | null {
		return this.intent?.idempotencyKey ?? null;
	}

	/** Starts a NEW intent: a new key and new field/view ids. */
	async start(title: string, labels: CollectionEmbedLabels): Promise<CollectionEmbedOutcome> {
		if (this.busy) return { kind: 'busy' };
		const validation = validateEmbedTitle(title);
		if (validation !== 'ok') return { kind: 'invalid', reason: validation };
		const trimmed = title.trim();
		this.intent = {
			idempotencyKey: this.mintId(),
			title: trimmed,
			payload: {
				title: trimmed,
				initial_fields: [
					{
						field_id: this.mintId(),
						label: labels.fieldLabel.trim() || trimmed,
						field_type: 'text',
						config: {},
						index: 0
					}
				],
				initial_view: {
					view_id: this.mintId(),
					name: labels.viewName.trim() || trimmed,
					view_type: 'table',
					config: {},
					index: 0
				}
			}
		};
		return this.send();
	}

	/** Re-sends the failed intent: same key, same body. */
	async retry(): Promise<CollectionEmbedOutcome> {
		if (this.busy) return { kind: 'busy' };
		if (!this.intent || !this.failure?.retryable) {
			return {
				kind: 'failed',
				failure: this.failure ?? {
					key: COLLECTION_EMBED_FAILED_KEY,
					code: 'none',
					retryable: false
				}
			};
		}
		return this.send();
	}

	/** Drops a failed intent (the user gave up). */
	dismiss(): void {
		if (this.busy) return;
		this.intent = null;
		this.failure = null;
		this.emit();
	}

	/** Called whenever the Page document's blocks change: once the accepted embed block is in
	 * the document, the provisional block has done its job. */
	settle(documentBlockIds: ReadonlySet<string>): void {
		const provisional = this.provisional;
		if (!provisional || provisional.status !== 'accepted') return;
		if (provisional.blockId === null || documentBlockIds.has(provisional.blockId)) {
			this.provisional = null;
			this.emit();
		}
	}

	private async send(): Promise<CollectionEmbedOutcome> {
		const intent = this.intent;
		if (!intent) return { kind: 'busy' };
		this.busy = true;
		this.failure = null;
		this.provisional = {
			localId: intent.idempotencyKey,
			title: intent.title,
			status: 'pending',
			collectionId: null,
			blockId: null
		};
		this.emit();
		try {
			const change = await this.commands.execute(
				this.pageId,
				{ type: 'create_collection_embed', payload: intent.payload },
				{ idempotencyKey: intent.idempotencyKey }
			);
			const ids = embedResultIds(change);
			this.intent = null;
			this.provisional =
				ids.blockId === null
					? null
					: {
							localId: intent.idempotencyKey,
							title: intent.title,
							status: 'accepted',
							collectionId: ids.collectionId,
							blockId: ids.blockId
						};
			return { kind: 'created', ...ids };
		} catch (raw) {
			// The whole provisional block goes away; the intent (and its key) stays for retry.
			this.provisional = null;
			this.failure = failureFrom(raw, COLLECTION_EMBED_FAILED_KEY);
			return { kind: 'failed', failure: this.failure };
		} finally {
			this.busy = false;
			this.emit();
		}
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}

// ---- Page document blocks ---------------------------------------------------------------------

/** `embed_type` of the Page placeholder that points at a bridge reference. */
export const REFERENCE_EMBED_TYPE = 'forms_reference';

export type ReferenceMode = 'reference' | 'embed';

export type PageBlock =
	| { readonly kind: 'collection'; readonly blockId: string; readonly collectionId: string }
	| {
			readonly kind: 'reference';
			readonly blockId: string;
			readonly referenceId: string;
			readonly mode: ReferenceMode;
	  };

/** The structural slice of a Loro tree node this module reads (`loro-crdt` `LoroTreeNode`). */
export interface PageTreeNode {
	readonly id: unknown;
	readonly data: { get(key: string): unknown; set(key: string, value: unknown): unknown };
}

/** The structural slice of the Page document's `tree` container (`loro-crdt` `LoroTree`) --
 * the server's block tree (`crates/collab-core` `TREE_CONTAINER`). */
export interface PageTree {
	roots(): PageTreeNode[];
	createNode(parent?: undefined): PageTreeNode;
	delete(target: never): void;
}

export interface PageDocument {
	getTree(name: string): PageTree;
	subscribe(listener: () => void): () => void;
	commit(): void;
}

function text(value: unknown): string | null {
	return typeof value === 'string' && value.length > 0 ? value : null;
}

/** The Page's embed and reference blocks, in document order. Node metadata follows
 * `collab-core::engine`: `logical_id`, `kind`, and `prop:<key>` per block property. */
export function readPageBlocks(tree: PageTree): PageBlock[] {
	const out: PageBlock[] = [];
	for (const node of tree.roots()) {
		const blockId = text(node.data.get('logical_id'));
		const kind = text(node.data.get('kind'));
		if (!blockId || (kind !== null && kind !== 'block')) continue;
		const embedType = text(node.data.get('prop:embed_type'));
		if (embedType === 'collection') {
			const collectionId = text(node.data.get('prop:embed_object_id'));
			if (collectionId) out.push({ kind: 'collection', blockId, collectionId });
		} else if (embedType === REFERENCE_EMBED_TYPE) {
			const referenceId = text(node.data.get('prop:reference_id'));
			const mode = node.data.get('prop:display_mode') === 'embed' ? 'embed' : 'reference';
			if (referenceId) out.push({ kind: 'reference', blockId, referenceId, mode });
		}
	}
	return out;
}

/**
 * The Page's block tree as the canvas sees it. Reads come from the open document (which the
 * server's accepted snapshot / resync keeps current); the only writes are the reference
 * placeholders, which travel through the same `subscribeLocalUpdates` -> `ObjectSession` path as
 * every other local edit of the document.
 */
export class PageBlockTree {
	constructor(
		private readonly doc: PageDocument,
		private readonly mintId: () => string = newIdempotencyKey
	) {}

	private tree(): PageTree {
		return this.doc.getTree('tree');
	}

	blocks(): PageBlock[] {
		return readPageBlocks(this.tree());
	}

	subscribe(listener: (blocks: PageBlock[]) => void): () => void {
		listener(this.blocks());
		return this.doc.subscribe(() => listener(this.blocks()));
	}

	/** Appends a placeholder for `referenceId` unless one is already there (a replayed create
	 * returns the same id). Only ids and the display mode are written -- never Forms content. */
	addReferencePlaceholder(referenceId: string, mode: ReferenceMode): boolean {
		if (this.blocks().some((b) => b.kind === 'reference' && b.referenceId === referenceId)) {
			return false;
		}
		const node = this.tree().createNode(undefined);
		node.data.set('logical_id', this.mintId());
		node.data.set('kind', 'block');
		node.data.set('prop:embed_type', REFERENCE_EMBED_TYPE);
		node.data.set('prop:reference_id', referenceId);
		node.data.set('prop:display_mode', mode);
		this.doc.commit();
		return true;
	}

	/** Deletes every placeholder that points at `referenceId`. */
	removeReferencePlaceholders(referenceId: string): number {
		const tree = this.tree();
		const doomed = tree
			.roots()
			.filter(
				(node) =>
					node.data.get('prop:embed_type') === REFERENCE_EMBED_TYPE &&
					node.data.get('prop:reference_id') === referenceId
			);
		for (const node of doomed) tree.delete(node.id as never);
		if (doomed.length > 0) this.doc.commit();
		return doomed.length;
	}
}

// ---- cards the canvas renders -----------------------------------------------------------------

export type AvailableReference = Extract<FlowReferenceItem, { visibility: 'available' }>;

export type PageCard =
	| { readonly kind: 'collection'; readonly blockId: string; readonly collectionId: string }
	| {
			readonly kind: 'reference';
			readonly key: string;
			readonly mode: ReferenceMode;
			readonly blockId: string | null;
			readonly reference: AvailableReference;
	  }
	| {
			readonly kind: 'unavailable';
			readonly key: string;
			readonly blockId: string;
			readonly referenceId: string;
	  }
	| { readonly kind: 'provisional'; readonly provisional: ProvisionalEmbed };

function modeOf(reference: AvailableReference): ReferenceMode {
	return reference.display?.mode === 'embed' ? 'embed' : 'reference';
}

/**
 * Merges the document blocks, the bridge's live references and the provisional block into the
 * list the canvas shows, in document order. A placeholder whose reference the bridge no longer
 * returns (removed, archived target, or no longer readable) is an `unavailable` card. A live
 * reference without a placeholder (created from MCP / CLI) is appended. A provisional block is
 * shown only until its accepted block is in the document.
 */
export function buildPageCards(
	blocks: readonly PageBlock[],
	references: readonly FlowReferenceItem[],
	provisional: ProvisionalEmbed | null
): PageCard[] {
	const available = new Map<string, AvailableReference>();
	for (const item of references) {
		if (item.visibility === 'available') available.set(item.reference_id, item);
	}
	const placed = new Set<string>();
	const cards: PageCard[] = [];
	const blockIds = new Set<string>();
	for (const block of blocks) {
		blockIds.add(block.blockId);
		if (block.kind === 'collection') {
			cards.push(block);
			continue;
		}
		if (placed.has(block.referenceId)) continue;
		placed.add(block.referenceId);
		const reference = available.get(block.referenceId);
		cards.push(
			reference
				? {
						kind: 'reference',
						key: block.blockId,
						mode: modeOf(reference),
						blockId: block.blockId,
						reference
					}
				: {
						kind: 'unavailable',
						key: block.blockId,
						blockId: block.blockId,
						referenceId: block.referenceId
					}
		);
	}
	for (const reference of available.values()) {
		if (placed.has(reference.reference_id)) continue;
		cards.push({
			kind: 'reference',
			key: reference.reference_id,
			mode: modeOf(reference),
			blockId: null,
			reference
		});
	}
	if (provisional && !(provisional.blockId && blockIds.has(provisional.blockId))) {
		cards.push({ kind: 'provisional', provisional });
	}
	return cards;
}

// ---- Reference / Embed create and remove ------------------------------------------------------

export const REFERENCE_FAILED_KEY = 'flow.bridge.card.createFailed';
export const REFERENCE_REMOVE_FAILED_KEY = 'flow.bridge.card.removeFailed';
export const REFERENCES_LOAD_FAILED_KEY = 'flow.bridge.card.loadFailed';

export interface ReferenceTarget {
	readonly targetType: 'form' | 'form_record';
	readonly targetId: string;
}

export interface ReferenceCommands {
	createReference(
		objectId: string,
		input: {
			target_type: 'form' | 'form_record';
			target_id: string;
			display: { mode: ReferenceMode };
			idempotency_key: string;
		}
	): Promise<ApiResult<FlowReferenceReceipt>>;
	removeReference(
		objectId: string,
		referenceId: string,
		idempotencyKey: string
	): Promise<ApiResult<{ removed: boolean; event_id: string }>>;
}

export interface ReferenceReads {
	listReferences(objectId: string): Promise<ApiResult<FlowReferenceList>>;
}

export type CreateReferenceOutcome =
	| {
			readonly kind: 'created';
			readonly receipt: FlowReferenceReceipt;
			readonly mode: ReferenceMode;
	  }
	| { readonly kind: 'failed'; readonly failure: SlashFailure }
	| { readonly kind: 'busy' };

export type RemoveReferenceOutcome =
	| { readonly kind: 'removed' }
	| { readonly kind: 'needs_confirmation' }
	| { readonly kind: 'failed'; readonly failure: SlashFailure }
	| { readonly kind: 'busy' };

export class BridgeReferenceSession {
	private createIntent: { signature: string; key: string } | null = null;
	private removeIntents = new Map<string, string>();
	private busy = false;

	constructor(
		private readonly pageId: string,
		private readonly commands: ReferenceCommands,
		private readonly reads: ReferenceReads,
		private readonly mintId: () => string = newIdempotencyKey
	) {}

	/** The key a create of `target` in `mode` would send: the same target and mode reuse it
	 * until that create succeeds. */
	createKeyFor(target: ReferenceTarget, mode: ReferenceMode): string {
		const signature = JSON.stringify([target.targetType, target.targetId, mode]);
		if (!this.createIntent || this.createIntent.signature !== signature) {
			this.createIntent = { signature, key: this.mintId() };
		}
		return this.createIntent.key;
	}

	async create(target: ReferenceTarget, mode: ReferenceMode): Promise<CreateReferenceOutcome> {
		if (this.busy) return { kind: 'busy' };
		const key = this.createKeyFor(target, mode);
		this.busy = true;
		try {
			const result = await this.commands.createReference(this.pageId, {
				target_type: target.targetType,
				target_id: target.targetId,
				display: { mode },
				idempotency_key: key
			});
			if (result.code !== 0 || !result.data) {
				return { kind: 'failed', failure: failureFromEnvelope(result, REFERENCE_FAILED_KEY) };
			}
			this.createIntent = null;
			return { kind: 'created', receipt: result.data, mode };
		} finally {
			this.busy = false;
		}
	}

	/** Removes the reference -- only after the user confirmed (`confirmed` comes from their
	 * action, never a default). Only `DELETE .../references/{id}` is sent: the Forms target stays
	 * exactly as it is. A retry of the same removal reuses its key. */
	async remove(referenceId: string, confirmed: boolean): Promise<RemoveReferenceOutcome> {
		if (confirmed !== true) return { kind: 'needs_confirmation' };
		if (this.busy) return { kind: 'busy' };
		let key = this.removeIntents.get(referenceId);
		if (!key) {
			key = this.mintId();
			this.removeIntents.set(referenceId, key);
		}
		this.busy = true;
		try {
			const result = await this.commands.removeReference(this.pageId, referenceId, key);
			if (result.code !== 0 || !result.data) {
				return {
					kind: 'failed',
					failure: failureFromEnvelope(result, REFERENCE_REMOVE_FAILED_KEY)
				};
			}
			this.removeIntents.delete(referenceId);
			return { kind: 'removed' };
		} finally {
			this.busy = false;
		}
	}

	async list(): Promise<
		| { readonly kind: 'ok'; readonly items: FlowReferenceItem[] }
		| { readonly kind: 'failed'; readonly failure: SlashFailure }
	> {
		const result = await this.reads.listReferences(this.pageId);
		if (result.code !== 0 || !result.data) {
			return { kind: 'failed', failure: failureFromEnvelope(result, REFERENCES_LOAD_FAILED_KEY) };
		}
		return { kind: 'ok', items: result.data.items };
	}
}

// ---- permission state and Embed content -------------------------------------------------------

/** Whether the caller may read the target at all (`form.view` in the request-time decision). */
export function canViewTarget(state: BridgePermissionState | null | undefined): boolean {
	return !!state && Array.isArray(state.actions) && state.actions.includes('form.view');
}

export type PermissionBadge = 'controlled' | 'read_only' | 'unconfigured' | 'no_access';

/** Literal keys (the i18n parity gate checks literal keys by name). */
export const PERMISSION_BADGE_KEYS: Readonly<Record<PermissionBadge, string>> = {
	controlled: 'flow.bridge.card.permission.controlled',
	read_only: 'flow.bridge.card.permission.read_only',
	unconfigured: 'flow.bridge.card.permission.unconfigured',
	no_access: 'flow.bridge.card.permission.no_access'
};

export const REFERENCE_MODE_KEYS: Readonly<Record<ReferenceMode, string>> = {
	reference: 'flow.bridge.card.mode.reference',
	embed: 'flow.bridge.card.mode.embed'
};

export const TARGET_TYPE_KEYS: Readonly<Record<'form' | 'form_record', string>> = {
	form: 'flow.bridge.card.targetType.form',
	form_record: 'flow.bridge.card.targetType.form_record'
};

/** The badge an i18n key is chosen from (`flow.bridge.card.permission.<badge>`). */
export function permissionBadge(state: BridgePermissionState | null | undefined): PermissionBadge {
	if (!canViewTarget(state)) return 'no_access';
	if (state?.configuration === 'unconfigured') return 'unconfigured';
	return state?.access === 'controlled' ? 'controlled' : 'read_only';
}

export interface EmbedFormsReads {
	getForm(formId: string): Promise<ApiResult<UniversalForm>>;
	getRecord(recordId: string): Promise<ApiResult<FormRecord>>;
}

export interface EmbedField {
	readonly label: string;
	readonly value: string;
}

export type EmbedContent =
	| { readonly kind: 'restricted' }
	| { readonly kind: 'failed' }
	| {
			readonly kind: 'form';
			readonly projectId: string;
			readonly formId: string;
			readonly name: string;
			readonly description: string;
			readonly fieldCount: number;
	  }
	| {
			readonly kind: 'record';
			readonly projectId: string;
			readonly formId: string;
			readonly title: string;
			readonly fields: readonly EmbedField[];
	  };

/** The production Forms reads behind a card (the target module's own endpoints, under the
 * caller's own Forms permissions). */
export const FORMS_EMBED_READS: EmbedFormsReads = {
	getForm: (formId) => formsApi.get(formId),
	getRecord: (recordId) => formsApi.getRecord(recordId)
};

function formFields(form: UniversalForm | null): FormField[] {
	const fields = (form?.schema as { fields?: unknown } | undefined)?.fields;
	return Array.isArray(fields) ? (fields as FormField[]) : [];
}

function displayFormValue(value: unknown): string {
	if (value === null || value === undefined) return '';
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	if (Array.isArray(value)) return value.map(displayFormValue).filter(Boolean).join(', ');
	return JSON.stringify(value);
}

/**
 * Loads a card's target from Forms: the project/form ids its source link needs and, for an
 * Embed, the content it renders. Without `form.view` in the request-time `permission_state` it
 * returns `restricted` WITHOUT asking Forms for anything -- the placeholder is all a caller
 * without access may see.
 */
export async function loadEmbedContent(
	reference: AvailableReference,
	reads: EmbedFormsReads
): Promise<EmbedContent> {
	if (!canViewTarget(reference.permission_state)) return { kind: 'restricted' };
	if (reference.target_type === 'form') {
		const form = await reads.getForm(reference.target_id);
		if (form.code !== 0 || !form.data) return { kind: 'failed' };
		return {
			kind: 'form',
			projectId: form.data.project_id,
			formId: form.data.id,
			name: form.data.name,
			description: form.data.description ?? '',
			fieldCount: formFields(form.data).length
		};
	}
	const record = await reads.getRecord(reference.target_id);
	if (record.code !== 0 || !record.data) return { kind: 'failed' };
	const form = await reads.getForm(record.data.form_id);
	const labels = new Map(
		formFields(form.code === 0 ? form.data : null).map((field) => [
			field.key,
			field.label ?? field.key
		])
	);
	const fields = Object.entries(record.data.values ?? {})
		.map(([key, value]) => ({ label: labels.get(key) ?? key, value: displayFormValue(value) }))
		.filter((field) => field.value.length > 0);
	return {
		kind: 'record',
		projectId: record.data.project_id,
		formId: record.data.form_id,
		title: record.data.title,
		fields
	};
}

// ---- Forms target picker ----------------------------------------------------------------------

export interface PickerProject {
	readonly id: string;
	readonly label: string;
}

export interface FormsPickerReads {
	listProjects(workspaceId: string): Promise<PickerProject[]>;
	listForms(projectId: string): Promise<ApiResult<PaginatedData<UniversalForm>>>;
	listRecords(formId: string): Promise<ApiResult<PaginatedData<FormRecord>>>;
}

export const FORMS_PICKER_READS: FormsPickerReads = {
	listProjects: listWorkspaceProjects,
	listForms: (projectId) => formsApi.list(projectId, { per_page: 100 }),
	listRecords: (formId) => formsApi.listRecords(formId, { per_page: 50 })
};

export type PickerLoad = 'idle' | 'loading' | 'ready' | 'failed';

export interface FormsPickerSnapshot {
	readonly projects: readonly PickerProject[];
	readonly projectsState: PickerLoad;
	readonly projectId: string | null;
	readonly forms: readonly UniversalForm[];
	readonly formsState: PickerLoad;
	readonly formId: string | null;
	readonly records: readonly FormRecord[];
	readonly recordsState: PickerLoad;
	/** `null` picks the form itself. */
	readonly recordId: string | null;
	readonly target: ReferenceTarget | null;
}

/** project -> form -> (optional) record. Each step's list is requested only once its parent is
 * chosen; a late answer for a parent the user already left is dropped. */
export class FormsTargetPicker {
	private projects: PickerProject[] = [];
	private projectsState: PickerLoad = 'idle';
	private projectId: string | null = null;
	private forms: UniversalForm[] = [];
	private formsState: PickerLoad = 'idle';
	private formId: string | null = null;
	private records: FormRecord[] = [];
	private recordsState: PickerLoad = 'idle';
	private recordId: string | null = null;

	constructor(
		private readonly workspaceId: string,
		private readonly reads: FormsPickerReads,
		private readonly onChange: (snapshot: FormsPickerSnapshot) => void = () => {}
	) {}

	snapshot(): FormsPickerSnapshot {
		const target: ReferenceTarget | null = this.recordId
			? { targetType: 'form_record', targetId: this.recordId }
			: this.formId
				? { targetType: 'form', targetId: this.formId }
				: null;
		return {
			projects: this.projects,
			projectsState: this.projectsState,
			projectId: this.projectId,
			forms: this.forms,
			formsState: this.formsState,
			formId: this.formId,
			records: this.records,
			recordsState: this.recordsState,
			recordId: this.recordId,
			target
		};
	}

	async load(): Promise<void> {
		this.projectsState = 'loading';
		this.emit();
		try {
			this.projects = await this.reads.listProjects(this.workspaceId);
			this.projectsState = 'ready';
		} catch {
			this.projectsState = 'failed';
		}
		this.emit();
	}

	async selectProject(projectId: string | null): Promise<void> {
		this.projectId = projectId;
		this.forms = [];
		this.formId = null;
		this.records = [];
		this.recordId = null;
		this.recordsState = 'idle';
		this.formsState = projectId ? 'loading' : 'idle';
		this.emit();
		if (!projectId) return;
		const result = await this.reads.listForms(projectId).catch(() => null);
		if (this.projectId !== projectId) return;
		if (result && result.code === 0 && result.data) {
			this.forms = result.data.items.filter((form) => !form.archived_at);
			this.formsState = 'ready';
		} else {
			this.formsState = 'failed';
		}
		this.emit();
	}

	async selectForm(formId: string | null): Promise<void> {
		this.formId = formId;
		this.records = [];
		this.recordId = null;
		this.recordsState = formId ? 'loading' : 'idle';
		this.emit();
		if (!formId) return;
		const result = await this.reads.listRecords(formId).catch(() => null);
		if (this.formId !== formId) return;
		if (result && result.code === 0 && result.data) {
			this.records = result.data.items.filter((record) => !record.archived_at);
			this.recordsState = 'ready';
		} else {
			this.recordsState = 'failed';
		}
		this.emit();
	}

	selectRecord(recordId: string | null): void {
		this.recordId = recordId && this.records.some((r) => r.id === recordId) ? recordId : null;
		this.emit();
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}
