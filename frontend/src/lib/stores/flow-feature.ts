// Per-workspace `flow_enabled` flag shared by the app sidebar (Flow nav entry) and the Flow
// settings page (`contracts/ui-surface-v1.md`: "flag false：导航不显示"). Fail closed: anything
// other than a successful read that says `flow_enabled: true` is `false` -- an error envelope,
// a missing `data`, a rejected promise, or a workspace that was never read.

import { writable, type Readable } from 'svelte/store';
import type { ApiResult } from '$lib/api/client';
import type { FlowFeatureFlags } from '$lib/api/flow';
import { FlowCommandService } from '$lib/flow/command-service';

export type FlowFeatureFetcher = (workspaceId: string) => Promise<ApiResult<FlowFeatureFlags>>;

export interface FlowFeatureStore extends Readable<Record<string, boolean>> {
	/** Reads the flag for `workspaceId` and records it; resolves to the recorded value. */
	refresh(workspaceId: string): Promise<boolean>;
	/** Records a flag value the caller already holds from a successful server response. */
	set(workspaceId: string, enabled: boolean): void;
	/** Synchronous read of the recorded value; `false` when never read. */
	enabled(workspaceId: string): boolean;
}

/** The fail-closed reading of one `GET .../features/flow` result. */
export function flowEnabledFromResult(result: ApiResult<FlowFeatureFlags> | null): boolean {
	if (!result || result.code !== 0 || !result.data) return false;
	return result.data.flow_enabled === true;
}

export function createFlowFeatureStore(fetcher: FlowFeatureFetcher): FlowFeatureStore {
	const state = writable<Record<string, boolean>>({});
	let snapshot: Record<string, boolean> = {};
	state.subscribe((value) => {
		snapshot = value;
	});
	// Only the newest refresh per workspace may write, so a slow stale read cannot overwrite a
	// value the settings page has just recorded from its own successful PUT.
	const generation = new Map<string, number>();

	function record(workspaceId: string, enabled: boolean): void {
		state.update((current) => ({ ...current, [workspaceId]: enabled }));
	}

	return {
		subscribe: state.subscribe,
		async refresh(workspaceId: string): Promise<boolean> {
			const mine = (generation.get(workspaceId) ?? 0) + 1;
			generation.set(workspaceId, mine);
			let enabled = false;
			try {
				enabled = flowEnabledFromResult(await fetcher(workspaceId));
			} catch {
				enabled = false;
			}
			if (generation.get(workspaceId) === mine) record(workspaceId, enabled);
			return snapshot[workspaceId] === true;
		},
		set(workspaceId: string, enabled: boolean): void {
			generation.set(workspaceId, (generation.get(workspaceId) ?? 0) + 1);
			record(workspaceId, enabled);
		},
		enabled(workspaceId: string): boolean {
			return snapshot[workspaceId] === true;
		}
	};
}

const commands = new FlowCommandService();

export const flowFeatureStore = createFlowFeatureStore((workspaceId) =>
	commands.getFlowFeature(workspaceId)
);
