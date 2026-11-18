/** Account- and original-Page-bound runtime. It pins authorized documents, never a fake DOM order. */
import { apiClient } from '#lib/api/client.js';
import type { FlowObjectRepository, OpenFlowObjectResult } from '../object-repository';
import type { FlowObjectView } from '#lib/api/flow.js';
import { sourceFrontierMatches } from '../shared-blocks/review';
import { clearEditorSessionHistory } from '../editor-session';
import { BlockTransferOperation, type TransferOperationHost } from './controller';
import { blockTransferTransport } from './transport';
import { planBlockTransfer, type TransferPageReadiness } from './plan';
import { transferPreviewBlocks, targetSiblingCount, type TransferPreviewBlock } from './native';
import type { BlockTransferKind } from './contracts';
export interface TransferRuntimePreview {
	source: FlowObjectView;
	target: FlowObjectView;
	sourceBlocks: TransferPreviewBlock[];
	targetBlocks: TransferPreviewBlock[];
	selected: TransferPreviewBlock[];
}
export class TransferPageRuntime {
	readonly epoch = apiClient.getAuthSessionEpoch();
	private readonly abort = new AbortController();
	private readonly pins = new Map<string, OpenFlowObjectResult>();
	private readonly lockReleases = new Set<() => void>();
	private disposed = false;
	readonly operation: BlockTransferOperation;
	persistCheckpoint: (() => void) | null = null;
	preview: TransferRuntimePreview | null = null;
	constructor(
		readonly sourceId: string,
		private readonly repo: FlowObjectRepository,
		private readonly actorId: string,
		private readonly currentActor: () => string | null
	) {
		const host: TransferOperationHost = {
			allowed: () => this.allowed(),
			hold: async (pages, mode) => {
				if (mode !== 'resolve' && pages.some((id) => repo.transferEditingHeld(id))) return null;
				// Freeze writes before asynchronous outbox inspection: typing cannot race the last readiness check.
				const operationId = crypto.randomUUID(),
					release = repo.holdTransferEditing(pages, operationId);
				try {
					for (const id of pages) {
						const summary = await repo.transferPendingSummary(id);
						if (
							mode !== 'resolve' &&
							(!summary.known ||
								summary.pending ||
								summary.drafts ||
								summary.bodyJobs ||
								summary.enqueues ||
								summary.paused)
						) {
							release();
							return null;
						}
					}
					if (!this.allowed()) {
						release();
						return null;
					}
				} catch {
					release();
					return null;
				}

				this.lockReleases.add(release);
				return {
					applied: async (receipt, reset, review) => {
						if (!this.allowed()) throw new Error('session_changed');
						const changed =
							review.kind === 'copy'
								? [review.targetPageId]
								: [review.sourcePageId, review.targetPageId];
						if (receipt) {
							const openPages = changed.filter((id) => repo.openTransferEntry(id));
							const merged = await Promise.all(
								openPages.map((id) => repo.mergeTransferAccepted(id))
							);
							if (merged.some((ok) => !ok)) throw new Error('accepted_head_unavailable');
						}
						if (reset)
							for (const id of changed) {
								const entry = repo.openTransferEntry(id);
								if (entry) clearEditorSessionHistory(entry.doc);
							}
					},
					release: () => {
						release();
						this.lockReleases.delete(release);
					}
				};
			},
			reviewStillCurrent: (review) =>
				this.currentHeads(review.sourceFrontier, review.targetFrontier, review.targetPageId),
			inverseStillCurrent: (receipt) =>
				this.currentHeads(
					receipt.sourcePostFrontier,
					receipt.targetPostFrontier,
					receipt.targetPageId
				)
		};
		const transport = blockTransferTransport(this.epoch);
		for (const method of ['commit', 'undo'] as const) {
			const original = transport[method].bind(transport);
			if (method === 'commit')
				transport.commit = async (review, key, signal) => {
					try {
						this.persistCheckpoint?.();
					} catch {
						return { state: 'not_applied', reason: 'local_journal_unavailable' };
					}
					return (original as typeof transport.commit)(review, key, signal);
				};
			else
				transport.undo = async (receipt, key, signal) => {
					try {
						this.persistCheckpoint?.();
					} catch {
						return { state: 'not_applied', reason: 'local_journal_unavailable' };
					}
					return (original as typeof transport.undo)(receipt, key, signal);
				};
		}
		this.operation = new BlockTransferOperation(sourceId, host, transport);
	}
	allowed(): boolean {
		return (
			!this.disposed &&
			this.currentActor() === this.actorId &&
			apiClient.isAuthSessionEpoch(this.epoch)
		);
	}
	private async page(id: string): Promise<{
		object: FlowObjectView;
		entry: OpenFlowObjectResult;
		ready: TransferPageReadiness;
		blocks: TransferPreviewBlock[];
	}> {
		if (!this.allowed()) throw new Error('session_changed');
		if (this.repo.transferEditingHeld(id)) throw new Error('open_and_sync');
		const summary = await this.repo.transferPendingSummary(id);
		if (
			!summary.known ||
			summary.pending ||
			summary.drafts ||
			summary.bodyJobs ||
			summary.enqueues ||
			summary.paused
		)
			throw new Error('open_and_sync');
		const response = await this.repo.get(id);
		if (!response.data || response.code !== 0 || !this.allowed())
			throw new Error('page_unavailable');
		const object = response.data;
		let entry = this.pins.get(id);
		if (!entry) {
			entry = await this.repo.open({
				scopeId: object.scope_id,
				objectId: id,
				object,
				signal: this.abort.signal
			});
			if (!this.allowed()) {
				await this.repo.close(id);
				throw new Error('session_changed');
			}
			this.pins.set(id, entry);
		}
		const after = await this.repo.transferPendingSummary(id);
		return {
			object,
			entry,
			ready: {
				object,
				frontier: object.frontier,
				nativeVersionMatches: sourceFrontierMatches(entry.doc, object.frontier),
				pending: after.pending,
				persistedPending: after.pending,
				readinessKnown: after.known,
				localReplica: 'open',
				bodyJobs: after.bodyJobs,
				enqueuePending: after.enqueues > 0,
				paused: after.paused || after.drafts > 0
			},
			blocks: transferPreviewBlocks(entry.doc)
		};
	}
	async loadTarget(targetId: string): Promise<TransferRuntimePreview> {
		const [source, target] = await Promise.all([this.page(this.sourceId), this.page(targetId)]);
		if (source.object.scope_id !== target.object.scope_id) throw new Error('target_unavailable');
		this.preview = {
			source: source.object,
			target: target.object,
			sourceBlocks: source.blocks,
			targetBlocks: target.blocks,
			selected: []
		};
		return this.preview;
	}
	async prepare(input: {
		kind: BlockTransferKind;
		rootCid: string;
		selection: 'block' | 'current_subtree';
		targetId: string;
		parentCid: string | null;
		index?: number;
	}): Promise<boolean> {
		const [source, target] = await Promise.all([
			this.page(this.sourceId),
			this.page(input.targetId)
		]);
		if (source.object.scope_id !== target.object.scope_id) throw new Error('target_unavailable');
		const planned = planBlockTransfer({
			kind: input.kind,
			source: source.ready,
			target: target.ready,
			rootCid: input.rootCid,
			selection: input.selection,
			sourceBlocks: source.blocks,
			targetParentCid: input.parentCid,
			targetSiblingIndex: input.index ?? targetSiblingCount(target.blocks, input.parentCid)
		});
		const ids = new Set(planned.fixedPreview.map((block) => block.cid));
		this.preview = {
			source: source.object,
			target: target.object,
			sourceBlocks: source.blocks,
			targetBlocks: target.blocks,
			selected: source.blocks.filter((block) => ids.has(block.cid))
		};
		return this.operation.prepare(planned.request);
	}
	private async currentHeads(source: string, target: string, targetId: string): Promise<boolean> {
		try {
			const [a, b] = await Promise.all([this.page(this.sourceId), this.page(targetId)]);
			return (
				this.allowed() &&
				a.ready.frontier === source &&
				b.ready.frontier === target &&
				a.ready.nativeVersionMatches &&
				b.ready.nativeVersionMatches
			);
		} catch {
			return false;
		}
	}
	async releasePins(): Promise<void> {
		const ids = [...this.pins.keys()];
		this.pins.clear();
		for (const id of ids) await this.repo.close(id);
	}
	async dispose(): Promise<void> {
		this.disposed = true;
		this.abort.abort();
		this.operation.dispose();
		for (const release of this.lockReleases) release();
		this.lockReleases.clear();
		await this.releasePins();
	}
}
