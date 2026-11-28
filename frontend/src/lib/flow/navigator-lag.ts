// Workspace projection-lag badge at the top of the navigator (`surface-coverage-v1.md`:
// `GET /workspaces/{workspace_id}/flow/projection-lag` -> lag badge; `rest-api-v1.md` v0.5:
// `{max_lag,p95_lag,items,next_cursor?}`).
//
// Both scope-native and workspace adapters compute aggregates before pagination.
// Personal scope reads fail explicitly if the published authorization scan budget is exceeded.
// Refreshed every 30s while mounted. A rejected read hides the numbers rather than showing a
// stale "synced": the badge must never claim a state it did not observe.

import type { ApiResult } from '#lib/api/client.js';
import type { FlowProjectionLag } from '#lib/api/flow.js';
import { flowErrorFromEnvelope } from './errors';

export const LAG_REFRESH_MS = 30_000;

export type LagBadgeState =
	| { readonly kind: 'loading' }
	| { readonly kind: 'synced' }
	| { readonly kind: 'lagging'; readonly maxLag: number; readonly p95Lag: number }
	| { readonly kind: 'unavailable'; readonly errorCode: string | null };

export interface LagReads {
	getProjectionLag(scopeId: string): Promise<ApiResult<FlowProjectionLag>>;
}

export interface LagBadgeDeps {
	readonly reads: LagReads;
	readonly intervalMs: number;
	readonly setInterval: (callback: () => void, ms: number) => unknown;
	readonly clearInterval: (handle: unknown) => void;
}

export function lagBadgeFromResult(result: ApiResult<FlowProjectionLag>): LagBadgeState {
	if (result.code !== 0 || !result.data) {
		return { kind: 'unavailable', errorCode: flowErrorFromEnvelope(result)?.code ?? null };
	}
	const maxLag = Number(result.data.max_lag);
	const p95Lag = Number(result.data.p95_lag);
	if (!Number.isFinite(maxLag) || !Number.isFinite(p95Lag) || maxLag < 0 || p95Lag < 0) {
		return { kind: 'unavailable', errorCode: null };
	}
	return maxLag === 0 ? { kind: 'synced' } : { kind: 'lagging', maxLag, p95Lag };
}

export class FlowLagBadge {
	private readonly deps: LagBadgeDeps;
	private state: LagBadgeState = { kind: 'loading' };
	private handle: unknown = null;
	private generation = 0;

	constructor(
		/** A getter keeps a component from capturing its prop's first value. */
		private readonly workspace: string | (() => string),
		private readonly onChange: (state: LagBadgeState) => void = () => {},
		deps: Partial<LagBadgeDeps> & { reads: LagReads }
	) {
		this.deps = {
			reads: deps.reads,
			intervalMs: deps.intervalMs ?? LAG_REFRESH_MS,
			setInterval: deps.setInterval ?? ((callback, ms) => setInterval(callback, ms)),
			clearInterval:
				deps.clearInterval ?? ((handle) => clearInterval(handle as ReturnType<typeof setInterval>))
		};
	}

	private scopeId(): string {
		return typeof this.workspace === 'function' ? this.workspace() : this.workspace;
	}

	get current(): LagBadgeState {
		return this.state;
	}

	/** First read now, then one every `intervalMs` until `stop()`. */
	start(): void {
		this.stop();
		void this.refresh();
		this.handle = this.deps.setInterval(() => void this.refresh(), this.deps.intervalMs);
	}

	stop(): void {
		if (this.handle !== null) {
			this.deps.clearInterval(this.handle);
			this.handle = null;
		}
		this.generation += 1;
	}

	async refresh(): Promise<LagBadgeState> {
		const generation = this.generation;
		const result = await this.deps.reads.getProjectionLag(this.scopeId());
		if (generation !== this.generation) return this.state;
		this.state = lagBadgeFromResult(result);
		this.onChange(this.state);
		return this.state;
	}
}
