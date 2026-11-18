import { apiClient } from '#lib/api/client.js';
/** Source management remains tied to its original account and TaskTab, including legacy read views. */
import type { TaskWorkbench } from '../../workbench/tabs';
import type { FlowObjectRepository } from '../object-repository';
import { SourceBlockShares } from './source';
interface Binding {
	manager: SourceBlockShares;
	setManage(value: boolean): void;
}
const retained = new WeakMap<TaskWorkbench, Map<string, Binding>>();
export function retainedSourceShares(
	workbench: TaskWorkbench,
	repository: FlowObjectRepository,
	sourceId: string,
	canManage: boolean
): Binding | null {
	const owner = workbench.current.userId,
		key = `flow:${sourceId}`;
	if (!owner || !workbench.current.tabs.some((tab) => tab.key === key)) return null;
	let entries = retained.get(workbench);
	if (!entries) {
		entries = new Map();
		retained.set(workbench, entries);
	}
	const previous = entries.get(sourceId);
	if (previous) {
		previous.setManage(canManage);
		return previous;
	}
	const epoch = apiClient.getAuthSessionEpoch();
	let knownManage = canManage;
	const manager = new SourceBlockShares({
		sourceId,
		allowed: () =>
			apiClient.isAuthSessionEpoch(epoch) &&
			workbench.current.userId === owner &&
			workbench.current.tabs.some((tab) => tab.key === key) &&
			(repository.blockShareManagementAllowed(sourceId) ?? knownManage),
		review: (root, selection, signal) =>
			repository.reviewBlockShare(sourceId, root, selection, signal),
		current: (review, signal) => repository.blockShareReviewCurrent(sourceId, review, signal)
	});
	const binding: Binding = {
		manager,
		setManage(value) {
			knownManage = value;
		}
	};
	const stop = manager.subscribe(() => workbench.refreshAuxiliaryDirty(key));
	entries.set(sourceId, binding);
	if (
		!workbench.retainAuxiliary(key, 'block-sharing', {
			dirty: () => {
				const current = manager.current();
				return current.busy || current.dirty;
			},
			dispose: () => {
				stop();
				manager.dispose();
				entries!.delete(sourceId);
			}
		})
	) {
		stop();
		manager.dispose();
		entries.delete(sourceId);
		return null;
	}
	return binding;
}
