/** Account-owned operations survive Page components and TaskTab closure. */
import { apiClient } from '#lib/api/client.js';
import type { FlowObjectRepository } from '../object-repository';
import { TransferPageRuntime } from './runtime';
import { BlockTransferJournal } from './journal';
const retained = new WeakMap<FlowObjectRepository, TransferSessions>();
export class TransferSessions {
	readonly epoch = apiClient.getAuthSessionEpoch();
	private readonly runtimes = new Map<string, TransferPageRuntime>();
	private readonly stops: (() => void)[] = [];
	private readonly journal: BlockTransferJournal | null;
	private readonly listeners = new Set<() => void>();
	constructor(
		private repo: FlowObjectRepository,
		readonly actor: string,
		private currentActor: () => string | null
	) {
		this.journal =
			typeof localStorage !== 'undefined' ? new BlockTransferJournal(localStorage, actor) : null;
		for (const entry of this.journal?.list() ?? []) {
			const runtime = this.forSource(entry.sourcePageId);
			if (runtime.operation.current().phase !== 'idle') continue;
			runtime.operation.restore(entry.checkpoint);
			const state = runtime.operation.current();
			if (state.phase === 'unknown') {
				const release = repo.holdTransferEditing(
					[entry.sourcePageId, state.review!.targetPageId],
					`recovered:${state.review!.id}`
				);
				const stop = runtime.operation.subscribe((next) => {
					if (!['unknown', 'committing', 'undoing'].includes(next.phase)) release();
				});
				this.stops.push(() => {
					stop();
					release();
				});
			}
		}
		apiClient.authSessionSignal(this.epoch).addEventListener(
			'abort',
			() => {
				void this.dispose();
			},
			{ once: true }
		);
	}
	allowed(): boolean {
		return apiClient.isAuthSessionEpoch(this.epoch) && this.currentActor() === this.actor;
	}
	forSource(sourceId: string): TransferPageRuntime {
		if (!this.allowed()) throw new Error('session_changed');
		let runtime = this.runtimes.get(sourceId);
		if (!runtime) {
			runtime = new TransferPageRuntime(sourceId, this.repo, this.actor, this.currentActor);
			this.runtimes.set(sourceId, runtime);
			const owned = runtime;
			owned.persistCheckpoint = () => {
				if (!this.journal) throw new Error('local_journal_unavailable');
				this.journal.put({
					actorId: this.actor,
					sourcePageId: sourceId,
					checkpoint: owned.operation.checkpoint()
				});
			};
			this.stops.push(
				owned.operation.subscribe((state) => {
					if (state.review && !['idle', 'preparing', 'failed'].includes(state.phase)) {
						try {
							owned.persistCheckpoint?.();
						} catch {
							/* Outbound guard refuses writes when durability is unavailable. */
						}
					}
					for (const listener of this.listeners) listener();
				})
			);
		}
		return runtime;
	}
	forPage(pageId: string): TransferPageRuntime[] {
		return [...this.runtimes.values()].filter((runtime) => {
			const state = runtime.operation.current();
			return (
				state.review &&
				(runtime.sourceId === pageId || state.review.targetPageId === pageId) &&
				(['committing', 'unknown', 'undoing'].includes(state.phase) ||
					(state.phase === 'applied' && state.receipt !== null))
			);
		});
	}
	subscribe(listener: () => void): () => void {
		this.listeners.add(listener);
		listener();
		return () => {
			this.listeners.delete(listener);
		};
	}
	async dispose(): Promise<void> {
		for (const stop of this.stops) stop();
		this.stops.length = 0;
		this.listeners.clear();
		for (const runtime of this.runtimes.values()) await runtime.dispose();
		this.runtimes.clear();
		if (retained.get(this.repo) === this) retained.delete(this.repo);
	}
}
export function transferSessions(
	repo: FlowObjectRepository,
	actor: string,
	currentActor: () => string | null
): TransferSessions {
	const prior = retained.get(repo);
	if (prior?.allowed() && prior.actor === actor) return prior;
	if (prior) void prior.dispose();
	const sessions = new TransferSessions(repo, actor, currentActor);
	retained.set(repo, sessions);
	return sessions;
}
