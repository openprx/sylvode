/** Original-Page/account-bound source publication draft. Server review and writes stay explicit. */
import { randomUuid } from '#lib/utils/uuid.js';
import type { ApiResult } from '#lib/api/client.js';
import type { BlockShareReview } from './review';
import type { CreateBlockShare, ManagedBlockShare, SharedAudience } from './contracts';
import { sharedBlockApi } from './client';
export interface SourceShareForm {
	rootCid: string;
	selection: 'block' | 'current_subtree';
	workspaceId: string;
	projectId: string;
	audience: 'workspace' | 'users';
	recipients: string[];
	permission: 'view' | 'edit';
	republishId: string | null;
}
export interface SourceShareHost {
	sourceId: string;
	allowed(): boolean;
	review(
		root: string,
		selection: 'block' | 'current_subtree',
		signal: AbortSignal
	): Promise<{ ok: true; review: BlockShareReview } | { ok: false; reason: string }>;
	current(review: BlockShareReview, signal: AbortSignal): Promise<boolean>;
}
export interface SourceShareSnapshot {
	form: SourceShareForm;
	review: BlockShareReview | null;
	managed: ManagedBlockShare[];
	busy: boolean;
	dirty: boolean;
	status: 'idle' | 'reviewing' | 'saving' | 'uncertain' | 'rejected' | 'saved' | 'unavailable';
	error:
		| 'source_changed'
		| 'not_saved'
		| 'unsupported'
		| 'limit'
		| 'unavailable'
		| 'target'
		| 'failed'
		| null;
	intent: { key: string; payload: CreateBlockShare; republishId: string | null } | null;
}
const empty = (): SourceShareForm => ({
	rootCid: '',
	selection: 'block',
	workspaceId: '',
	projectId: '',
	audience: 'users',
	recipients: [],
	permission: 'view',
	republishId: null
});
export class SourceBlockShares {
	private value: SourceShareSnapshot = {
		form: empty(),
		review: null,
		managed: [],
		busy: false,
		dirty: false,
		status: 'idle',
		error: null,
		intent: null
	};
	private listeners = new Set<(value: SourceShareSnapshot) => void>();
	private disposed = false;
	private lifecycleKeys = new Map<string, string>();
	private abort = new AbortController();
	constructor(private readonly host: SourceShareHost) {}
	current(): SourceShareSnapshot {
		return structuredClone(this.value);
	}
	subscribe(listener: (value: SourceShareSnapshot) => void): () => void {
		this.listeners.add(listener);
		listener(this.current());
		return () => this.listeners.delete(listener);
	}
	private emit(): void {
		if (!this.disposed) for (const listener of this.listeners) listener(this.current());
	}
	private allowed(): boolean {
		return !this.disposed && this.host.allowed();
	}
	select(rootCid: string): void {
		if (this.value.busy || this.value.dirty) return;
		this.value.form = { ...empty(), rootCid };
		this.value.review = null;
		this.value.error = null;
		this.value.status = 'idle';
		this.emit();
	}
	edit(patch: Partial<SourceShareForm>): boolean {
		if (this.value.busy || this.value.status === 'uncertain' || !this.allowed()) return false;
		if (
			this.value.form.republishId &&
			((patch.workspaceId !== undefined && patch.workspaceId !== this.value.form.workspaceId) ||
				(patch.projectId !== undefined && patch.projectId !== this.value.form.projectId))
		)
			return false;
		this.value.form = { ...this.value.form, ...structuredClone(patch) };
		this.value.review = null;
		this.value.intent = null;
		this.value.dirty = true;
		this.value.status = 'idle';
		this.value.error = null;
		this.emit();
		return true;
	}
	republish(share: ManagedBlockShare): boolean {
		if (this.value.busy || this.value.dirty || !this.allowed()) return false;
		const target = share.audiences[0];
		this.value.form = {
			rootCid: share.root_block_cid,
			selection: share.selection,
			workspaceId: target.workspace_id,
			projectId: target.project_id ?? '',
			audience: target.audience,
			recipients: [...(target.recipient_user_ids ?? [])],
			permission: target.permission,
			republishId: share.share_id
		};
		this.value.review = null;
		this.value.intent = null;
		this.value.status = 'idle';
		this.value.dirty = true;
		this.value.error = null;
		this.emit();
		return true;
	}
	async list(): Promise<void> {
		if (!this.allowed()) return;
		const result = await sharedBlockApi.manage(this.host.sourceId);
		if (this.disposed) return;
		if (result.code === 0 && result.data) this.value.managed = result.data;
		else {
			this.value.managed = [];
			this.value.error = 'unavailable';
		}
		this.emit();
	}
	private audience(): SharedAudience | null {
		const f = this.value.form,
			uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
		if (
			!uuid.test(f.workspaceId) ||
			(f.projectId && !uuid.test(f.projectId)) ||
			(f.projectId && f.audience !== 'users') ||
			(f.audience === 'users' &&
				(!f.recipients.length ||
					f.recipients.length > 128 ||
					new Set(f.recipients).size !== f.recipients.length ||
					f.recipients.some((id) => !uuid.test(id))))
		)
			return null;
		return {
			workspace_id: f.workspaceId,
			...(f.projectId ? { project_id: f.projectId } : {}),
			audience: f.audience,
			...(f.audience === 'users' ? { recipient_user_ids: [...f.recipients] } : {}),
			permission: f.permission
		};
	}
	async review(): Promise<boolean> {
		if (
			!this.allowed() ||
			this.value.busy ||
			this.value.status === 'uncertain' ||
			!this.value.form.rootCid ||
			!this.audience()
		) {
			this.value.error = 'target';
			this.emit();
			return false;
		}
		this.value.busy = true;
		this.value.status = 'reviewing';
		this.value.error = null;
		this.emit();
		const f = structuredClone(this.value.form);
		let result: Awaited<ReturnType<SourceShareHost['review']>>;
		try {
			result = await this.host.review(f.rootCid, f.selection, this.abort.signal);
		} catch {
			result = { ok: false, reason: 'unavailable' };
		}
		if (this.disposed) return false;
		this.value.busy = false;
		if (!this.allowed()) {
			this.value.review = null;
			this.value.managed = [];
			this.value.status = 'unavailable';
			this.value.error = 'unavailable';
			this.emit();
			return false;
		}
		if (!result.ok) {
			this.value.review = null;
			this.value.status = 'rejected';
			this.value.error = ['source_changed', 'not_saved', 'unsupported', 'limit'].includes(
				result.reason
			)
				? (result.reason as 'source_changed' | 'not_saved' | 'unsupported' | 'limit')
				: 'unavailable';
			this.emit();
			return false;
		}
		this.value.review = result.review;
		this.value.intent = null;
		this.value.status = 'idle';
		this.value.dirty = true;
		this.emit();
		return true;
	}
	async publish(): Promise<boolean> {
		if (!this.allowed() || this.value.busy || !this.value.review) return false;
		const audience = this.audience();
		if (!audience) return false;
		const review = this.value.review;
		const wasUncertain = this.value.status === 'uncertain';
		const payload: CreateBlockShare = {
			root_block_cid: review.root_block_cid,
			selection: review.selection,
			expected_frontier: review.expected_frontier,
			audiences: [audience]
		};
		if (!this.value.intent)
			this.value.intent = {
				key: randomUuid(),
				payload: structuredClone(payload),
				republishId: this.value.form.republishId
			};
		const intent = this.value.intent;
		if (JSON.stringify(intent.payload) !== JSON.stringify(payload)) return false;
		this.value.busy = true;
		this.value.status = 'saving';
		this.value.error = null;
		this.emit();
		let current = wasUncertain;
		if (!wasUncertain) {
			try {
				current = await this.host.current(review, this.abort.signal);
			} catch {
				current = false;
			}
		}
		if (!current) {
			if (this.disposed) return false;
			this.value.busy = false;
			this.value.status = 'rejected';
			this.value.error = 'source_changed';
			this.value.review = null;
			this.value.intent = null;
			this.emit();
			return false;
		}
		let result: ApiResult<ManagedBlockShare>;
		try {
			result = intent.republishId
				? await sharedBlockApi.republish(
						this.host.sourceId,
						intent.republishId,
						intent.payload,
						intent.key
					)
				: await sharedBlockApi.create(this.host.sourceId, intent.payload, intent.key);
		} catch {
			result = { code: 500, message: 'failed', data: null };
		}
		if (this.disposed) return false;
		this.value.busy = false;
		if (result.code === 0 && result.data) {
			this.value.status = 'saved';
			this.value.dirty = false;
			this.value.review = null;
			this.value.intent = null;
			this.value.form = { ...empty(), rootCid: review.root_block_cid };
			await this.list();
			this.emit();
			return true;
		}
		this.value.status =
			wasUncertain || result.code === 499 || result.code >= 500 ? 'uncertain' : 'rejected';
		this.value.error = result.code === 403 || result.code === 404 ? 'unavailable' : 'failed';
		this.value.dirty = true;
		if (!wasUncertain && result.code !== 499 && result.code < 500) {
			this.value.review = null;
			this.value.intent = null;
		}
		this.emit();
		return false;
	}
	async changeLifecycle(shareId: string, action: 'suspend' | 'revoke'): Promise<boolean> {
		if (!this.allowed() || this.value.busy) return false;
		this.value.busy = true;
		this.value.error = null;
		this.emit();
		const operation = `${action}:${shareId}`;
		const key = this.lifecycleKeys.get(operation) ?? `${operation}:${randomUuid()}`;
		this.lifecycleKeys.set(operation, key);
		let result: ApiResult<ManagedBlockShare>;
		try {
			result = await sharedBlockApi.changeSourceLifecycle(this.host.sourceId, shareId, action, key);
		} catch {
			result = { code: 500, message: 'failed', data: null };
		}
		if (this.disposed) return false;
		if (result.code !== 0 || result.data?.share_id !== shareId) {
			this.value.busy = false;
			this.value.error = result.code === 403 || result.code === 404 ? 'unavailable' : 'failed';
			this.emit();
			return false;
		}
		this.lifecycleKeys.delete(operation);
		const updated = result.data;
		this.value.managed = this.value.managed.map((share) =>
			share.share_id === shareId ? updated : share
		);
		await this.list();
		this.value.busy = false;
		this.emit();
		return true;
	}

	discard(): void {
		if (this.value.busy) return;
		this.value.form = empty();
		this.value.review = null;
		this.value.intent = null;
		this.value.error = null;
		this.value.status = 'idle';
		this.value.dirty = false;
		this.emit();
	}
	dispose(): void {
		this.disposed = true;
		this.abort.abort();
		this.listeners.clear();
		this.value = {
			...this.value,
			review: null,
			managed: [],
			intent: null,
			form: empty(),
			dirty: false,
			busy: false
		};
	}
}
