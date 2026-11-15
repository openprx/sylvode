import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
import type { FlowObjectView } from '#lib/api/flow.js';
import type { TaskWorkbench } from './tabs';
export const FLOW_TASK_SESSIONS = 'sv-flow-task-sessions';
interface Pin {
	objectId: string;
	release(): void;
}
/** One repository per verified account, at most two resource slots per open task tab. */
export class FlowTaskSessions {
	readonly repository: FlowObjectRepository;
	private pins = new Map<string, Map<string, Pin>>();
	private disposed = false;
	constructor(
		readonly userId: string,
		private workbench: TaskWorkbench,
		repository: FlowObjectRepository
	) {
		this.repository = repository;
	}
	async retain(
		tabKey: string,
		slot: 'page' | 'navigator',
		scopeId: string,
		objectId: string,
		object?: FlowObjectView
	): Promise<void> {
		if (
			this.disposed ||
			this.workbench.current.userId !== this.userId ||
			!this.workbench.current.tabs.some((tab) => tab.key === tabKey)
		)
			return;
		let slots = this.pins.get(tabKey);
		if (slots?.get(slot)?.objectId === objectId) return;
		const opened = await this.repository.open({
			scopeId,
			objectId,
			object,
			signal: new AbortController().signal
		});
		if (
			this.disposed ||
			this.workbench.current.userId !== this.userId ||
			!this.workbench.current.tabs.some((tab) => tab.key === tabKey)
		) {
			await this.repository.close(objectId);
			return;
		}
		if (!slots) {
			slots = new Map();
			this.pins.set(tabKey, slots);
			this.workbench.retain(tabKey, () => this.release(tabKey));
		}
		slots.get(slot)?.release();
		let initialized = false;
		let sequence = -1;
		const unsubscribeProjection = this.repository
			.getProjection(objectId)
			.subscribe((projection) => {
				if (slot === 'page' && projection.title) this.workbench.setTitle(tabKey, projection.title);
				if (
					initialized &&
					projection.documentSeq > sequence &&
					this.workbench.current.active !== tabKey
				)
					this.workbench.markChanged(tabKey);
				sequence = projection.documentSeq;
				initialized = true;
			});
		const unsubscribePending =
			slot === 'page'
				? opened.pending.subscribe((count) => this.workbench.markDirty(tabKey, count > 0))
				: () => {};
		let unsubscribeDoc = () => {};
		const unsubscribeGeneration =
			slot === 'page'
				? opened.docGeneration.subscribe(() => {
						unsubscribeDoc();
						unsubscribeDoc = opened.doc.subscribe((event) => {
							if (event.by === 'local') this.workbench.markDirty(tabKey, true);
						});
					})
				: () => {};

		slots.set(slot, {
			objectId,
			release: () => {
				unsubscribeProjection();
				unsubscribePending();
				unsubscribeGeneration();
				unsubscribeDoc();
				void this.repository.close(objectId);
			}
		});
	}
	private release(key: string): void {
		const slots = this.pins.get(key);
		if (!slots) return;
		this.pins.delete(key);
		for (const pin of slots.values()) pin.release();
	}
	dispose(): void {
		if (this.disposed) return;
		this.disposed = true;
		for (const key of [...this.pins.keys()]) this.release(key);
		void this.repository.disposeAll();
	}
}
