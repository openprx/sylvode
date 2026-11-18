/** Normalized operation model; only the wire adapter may turn an authoritative response into a decision. */
import type { BlockTransferKind, PrepareBlockTransfer } from './contracts';
export interface ReviewedBlockTransfer {
	id: string;
	kind: BlockTransferKind;
	sourcePageId: string;
	targetPageId: string;
	sourceFrontier: string;
	targetFrontier: string;
	digest: string;
	expiresAt: number;
	memberCount: number;
	assetCount: number;
	affectedShareCount: number;
	assetsReady: boolean;
}
export interface BlockTransferReceipt {
	id: string;
	kind: BlockTransferKind;
	sourcePageId: string;
	targetPageId: string;
	sourcePostFrontier: string;
	targetPostFrontier: string;
	firstTargetBlockCid: string | null;
}
export type TransferDecision =
	| {
			state: 'applied' | 'undone';
			id: string;
			kind: BlockTransferKind;
			receipt: BlockTransferReceipt | null;
	  }
	| { state: 'not_applied'; reason: string }
	| { state: 'pending' | 'unknown' };
export interface TransferOperationTransport {
	remember?(review: ReviewedBlockTransfer): void;
	prepare(
		sourcePageId: string,
		input: PrepareBlockTransfer,
		key: string,
		signal: AbortSignal
	): Promise<ReviewedBlockTransfer>;
	commit(
		review: ReviewedBlockTransfer,
		key: string,
		signal: AbortSignal
	): Promise<TransferDecision>;
	receipt(id: string, signal: AbortSignal): Promise<TransferDecision>;
	undo(receipt: BlockTransferReceipt, key: string, signal: AbortSignal): Promise<TransferDecision>;
	cancel(id: string, key: string, signal: AbortSignal): Promise<TransferDecision>;
}
export interface TransferLease {
	/** Null receipt means ack-only: clear the captured local boundary if eligible, never fetch a private fallback. */
	applied(
		receipt: BlockTransferReceipt | null,
		resetOldNativeUndo: boolean,
		review: ReviewedBlockTransfer
	): Promise<void>;
	release(): void;
}
export interface TransferOperationHost {
	allowed(): boolean;
	hold(
		pageIds: readonly string[],
		mode: 'commit' | 'resolve' | 'undo'
	): Promise<TransferLease | null>;
	reviewStillCurrent(review: ReviewedBlockTransfer): Promise<boolean>;
	inverseStillCurrent(receipt: BlockTransferReceipt): Promise<boolean>;
}
export interface TransferOperationState {
	phase:
		| 'idle'
		| 'preparing'
		| 'prepared'
		| 'committing'
		| 'unknown'
		| 'applied'
		| 'not_applied'
		| 'undoing'
		| 'undone'
		| 'failed';
	review: ReviewedBlockTransfer | null;
	receipt: BlockTransferReceipt | null;
	detailsUnavailable: boolean;
	error:
		| 'review_changed'
		| 'dirty_pages'
		| 'confirmation_required'
		| 'request_failed'
		| 'undo_conflict'
		| 'receipt_mismatch'
		| null;
	commitKey: string;
	undoKey: string;
}
export interface TransferCheckpoint {
	state: TransferOperationState;
	action: 'commit' | 'undo';
}
export class BlockTransferOperation {
	private value: TransferOperationState = {
		phase: 'idle',
		review: null,
		receipt: null,
		detailsUnavailable: false,
		error: null,
		commitKey: crypto.randomUUID(),
		undoKey: crypto.randomUUID()
	};
	private prepareIntent: { signature: string; key: string } | null = null;
	private readonly cancelKey = crypto.randomUUID();
	private readonly abort = new AbortController();
	private action: 'commit' | 'undo' = 'commit';
	private actionStartedHere = false;
	private historySettled = false;
	private lease: TransferLease | null = null;
	private listeners = new Set<(value: TransferOperationState) => void>();
	private disposed = false;
	constructor(
		readonly sourcePageId: string,
		private readonly host: TransferOperationHost,
		private readonly transport: TransferOperationTransport
	) {}
	current(): TransferOperationState {
		return structuredClone(this.value);
	}
	subscribe(listener: (value: TransferOperationState) => void): () => void {
		this.listeners.add(listener);
		listener(this.current());
		return () => this.listeners.delete(listener);
	}
	private alive(): boolean {
		return !this.disposed && this.host.allowed();
	}
	private emit(): void {
		if (!this.disposed) for (const listener of this.listeners) listener(this.current());
	}
	private pages(): string[] {
		const review = this.value.review!;
		return [this.sourcePageId, review.targetPageId].sort();
	}
	async prepare(input: PrepareBlockTransfer): Promise<boolean> {
		if (!this.alive() || ['committing', 'unknown', 'undoing'].includes(this.value.phase))
			return false;
		const signature = JSON.stringify(input);
		if (
			this.prepareIntent?.signature !== signature ||
			(this.value.review && this.value.review.expiresAt <= Date.now())
		)
			this.prepareIntent = { signature, key: crypto.randomUUID() };
		this.value.phase = 'preparing';
		this.value.error = null;
		this.emit();
		try {
			const review = await this.transport.prepare(
				this.sourcePageId,
				input,
				this.prepareIntent!.key,
				this.abort.signal
			);
			if (!this.alive()) return false;
			if (
				review.sourcePageId !== this.sourcePageId ||
				review.targetPageId !== input.target_object_id ||
				review.kind !== input.kind ||
				review.sourceFrontier !== input.expected_source_frontier ||
				review.targetFrontier !== input.expected_target_frontier
			)
				throw new Error('review_mismatch');
			this.value.review = structuredClone(review);
			this.value.phase = 'prepared';
			this.value.commitKey = crypto.randomUUID();
			this.action = 'commit';
			this.actionStartedHere = false;
			this.historySettled = false;
			this.emit();
			return true;
		} catch {
			if (this.alive()) {
				this.value.phase = 'failed';
				this.value.error = 'request_failed';
				this.emit();
			}
			return false;
		}
	}
	async commit(acknowledgedHistoryBoundary: boolean): Promise<boolean> {
		const review = this.value.review;
		if (!this.alive() || this.value.phase !== 'prepared' || !review || !review.assetsReady)
			return false;
		if (!acknowledgedHistoryBoundary) {
			this.value.error = 'confirmation_required';
			this.emit();
			return false;
		}
		if (review.expiresAt <= Date.now() || !(await this.host.reviewStillCurrent(review))) {
			this.prepareIntent = null;
			this.value.error = 'review_changed';
			this.emit();
			return false;
		}
		this.lease = await this.host.hold(this.pages(), 'commit');
		if (!this.lease) {
			this.value.error = 'dirty_pages';
			this.emit();
			return false;
		}
		this.action = 'commit';
		this.actionStartedHere = true;
		this.historySettled = false;
		this.value.phase = 'committing';
		this.value.error = null;
		this.emit();
		return this.sendCurrent();
	}
	private async sendCurrent(): Promise<boolean> {
		let decision: TransferDecision;
		try {
			decision =
				this.action === 'undo'
					? await this.transport.undo(this.value.receipt!, this.value.undoKey, this.abort.signal)
					: await this.transport.commit(
							this.value.review!,
							this.value.commitKey,
							this.abort.signal
						);
		} catch {
			decision = { state: 'unknown' };
		}
		return this.decide(decision);
	}
	private receiptMatches(receipt: BlockTransferReceipt): boolean {
		const review = this.value.review!;
		return (
			receipt.id === review.id &&
			receipt.kind === review.kind &&
			receipt.sourcePageId === this.sourcePageId &&
			receipt.targetPageId === review.targetPageId
		);
	}
	private async decide(result: TransferDecision): Promise<boolean> {
		if (!this.alive()) return false;
		if (result.state === 'applied' || result.state === 'undone') {
			const review = this.value.review!;
			if (
				result.id !== review.id ||
				result.kind !== review.kind ||
				(result.receipt && !this.receiptMatches(result.receipt)) ||
				(this.action === 'undo' && result.state === 'applied')
			) {
				this.value.phase = 'unknown';
				this.value.error = 'receipt_mismatch';
				this.emit();
				return false;
			}
			try {
				await this.lease?.applied(
					result.receipt,
					this.actionStartedHere && !this.historySettled,
					review
				);
			} catch {
				this.value.phase = 'unknown';
				this.value.error = 'request_failed';
				this.emit();
				return false;
			}
			this.historySettled = true;
			this.value.receipt = result.receipt ? structuredClone(result.receipt) : null;
			this.value.detailsUnavailable = !result.receipt;
			this.value.phase = result.state;
			this.value.error = null;
			this.lease?.release();
			this.lease = null;
			this.emit();
			return true;
		}
		if (result.state === 'not_applied') {
			this.value.phase = this.action === 'undo' ? 'applied' : 'not_applied';
			this.value.error =
				this.action === 'undo'
					? 'undo_conflict'
					: ['transfer_review_stale', 'transfer_review_mismatch', 'expired'].includes(result.reason)
						? 'review_changed'
						: 'request_failed';
			if (this.action === 'commit') this.prepareIntent = null;
			this.lease?.release();
			this.lease = null;
			this.emit();
			return false;
		}
		this.value.phase = 'unknown';
		this.value.error = null;
		this.emit();
		return false;
	}
	async resolve(): Promise<boolean> {
		const review = this.value.review;
		if (!this.alive() || this.value.phase !== 'unknown' || !review) return false;
		if (!this.lease) this.lease = await this.host.hold(this.pages(), 'resolve');
		let result: TransferDecision;
		try {
			result = await this.transport.receipt(review.id, this.abort.signal);
		} catch {
			result = { state: 'unknown' };
		}
		return this.decide(result);
	}
	async retryUnknown(): Promise<boolean> {
		if (
			!this.alive() ||
			this.value.phase !== 'unknown' ||
			!this.value.review ||
			(this.action === 'undo' && !this.value.receipt)
		)
			return false;
		if (!this.lease) this.lease = await this.host.hold(this.pages(), 'resolve');
		this.value.phase = this.action === 'undo' ? 'undoing' : 'committing';
		this.emit();
		return this.sendCurrent();
	}
	async undo(): Promise<boolean> {
		const receipt = this.value.receipt;
		if (!this.alive() || this.value.phase !== 'applied' || !receipt) return false;
		if (!(await this.host.inverseStillCurrent(receipt))) {
			this.value.error = 'undo_conflict';
			this.emit();
			return false;
		}
		this.lease = await this.host.hold(this.pages(), 'undo');
		if (!this.lease) {
			this.value.error = 'dirty_pages';
			this.emit();
			return false;
		}
		this.action = 'undo';
		this.actionStartedHere = true;
		this.historySettled = false;
		this.value.phase = 'undoing';
		this.value.error = null;
		this.emit();
		return this.sendCurrent();
	}
	async cancel(): Promise<boolean> {
		if (
			!this.alive() ||
			!this.value.review ||
			this.action === 'undo' ||
			!['prepared', 'unknown'].includes(this.value.phase)
		)
			return false;
		const wasUnknown = this.value.phase === 'unknown';
		let result: TransferDecision;
		try {
			result = await this.transport.cancel(this.value.review.id, this.cancelKey, this.abort.signal);
		} catch {
			result = { state: 'unknown' };
		}
		if (result.state === 'unknown' && !wasUnknown) {
			this.value.error = 'request_failed';
			this.emit();
			return false;
		}
		return this.decide(result);
	}
	beginNewIntent(): boolean {
		if (['preparing', 'committing', 'unknown', 'undoing'].includes(this.value.phase)) return false;
		this.value = {
			phase: 'idle',
			review: null,
			receipt: null,
			detailsUnavailable: false,
			error: null,
			commitKey: crypto.randomUUID(),
			undoKey: crypto.randomUUID()
		};
		this.prepareIntent = null;
		this.action = 'commit';
		this.actionStartedHere = false;
		this.historySettled = false;
		this.emit();
		return true;
	}
	checkpoint(): TransferCheckpoint {
		return { state: this.current(), action: this.action };
	}
	restore(checkpoint: TransferCheckpoint): void {
		if (this.value.phase !== 'idle' || !checkpoint.state.review) return;
		this.value = structuredClone(checkpoint.state);
		this.action = checkpoint.action;
		this.actionStartedHere = false;
		this.transport.remember?.(this.value.review!);
		if (['committing', 'undoing'].includes(this.value.phase)) this.value.phase = 'unknown';
		this.emit();
	}
	/** A previous session's acknowledged operation must not clear new local native history. */
	recover(review: ReviewedBlockTransfer, commitKey: string): void {
		if (this.value.phase !== 'idle') return;
		this.actionStartedHere = false;
		this.value.review = structuredClone(review);
		this.transport.remember?.(review);
		this.value.commitKey = commitKey;
		this.value.phase = 'unknown';
		this.emit();
	}
	dispose(): void {
		this.disposed = true;
		this.abort.abort();
		this.listeners.clear(); /* Owner lifecycle releases its own Page locks; unknown is never reported as cancelled. */
	}
}
