/**
 * Lists the secondary column fetches from the existing API clients: workspaces, bots, AI
 * participants of a project, forms of a project. Each list loads and fails on its own, so one
 * failed list never blocks the rest of the column. Requests go through `ColumnApis` so tests
 * can pass fakes; `defaultColumnApis` (`column-apis.ts`) calls the real clients.
 */
import type { ApiResult, PaginatedData } from '#lib/api/client.js';
import type { AgentLike, BotLike } from './lists';
import type { WorkspaceRef } from './scope';

export type LoadStatus = 'idle' | 'loading' | 'ready' | 'error' | 'forbidden';

export interface Load<T> {
	status: LoadStatus;
	items: T[];
	/** Server total where the endpoint reports one. */
	total: number | null;
}

export interface FormRef {
	id: string;
	name: string;
}

export interface ColumnApis {
	workspaces(): Promise<ApiResult<PaginatedData<WorkspaceRef>>>;
	bots(workspaceId: string): Promise<ApiResult<BotLike[]>>;
	aiAgents(projectId: string): Promise<ApiResult<PaginatedData<AgentLike>>>;
	forms(projectId: string): Promise<ApiResult<PaginatedData<FormRef>>>;
}

export const IDLE: Load<never> = { status: 'idle', items: [], total: null };

/** A result's load state: 0 is ready, 403 is no permission, anything else failed. */
export function loadFromResult<T, U>(
	result: ApiResult<U> | null,
	pick: (data: U) => { items: T[]; total: number | null }
): Load<T> {
	if (!result) return { status: 'error', items: [], total: null };
	if (result.code === 0 && result.data) return { status: 'ready', ...pick(result.data) };
	return { status: result.code === 403 ? 'forbidden' : 'error', items: [], total: null };
}

const fromPage = <T>(page: PaginatedData<T>) => ({
	items: Array.isArray(page.items) ? page.items : [],
	total: typeof page.total === 'number' ? page.total : null
});

export class ColumnData {
	workspaces = $state.raw<Load<WorkspaceRef>>(IDLE);
	private bots = $state.raw<Record<string, Load<BotLike>>>({});
	private agents = $state.raw<Record<string, Load<AgentLike>>>({});
	private forms = $state.raw<Record<string, Load<FormRef>>>({});
	private apps = $state.raw<Record<string, number | null>>({});

	private generation: Record<string, number> = {};

	constructor(private readonly apis: ColumnApis) {}

	/** Runs one request; only the newest request for `key` may store its result. */
	private async run<T>(
		key: string,
		current: Load<T>,
		force: boolean,
		store: (value: Load<T>) => void,
		request: () => Promise<Load<T>>
	): Promise<void> {
		if (!force && (current.status === 'loading' || current.status === 'ready')) return;
		const mine = (this.generation[key] ?? 0) + 1;
		this.generation[key] = mine;
		store({ ...current, status: 'loading' });
		let value: Load<T>;
		try {
			value = await request();
		} catch {
			value = { status: 'error', items: [], total: null };
		}
		if (this.generation[key] === mine) store(value);
	}

	loadWorkspaces(force = false): Promise<void> {
		return this.run(
			'workspaces',
			this.workspaces,
			force,
			(v) => (this.workspaces = v),
			async () => loadFromResult(await this.apis.workspaces(), fromPage)
		);
	}

	botsOf(workspaceId: string): Load<BotLike> {
		return this.bots[workspaceId] ?? IDLE;
	}
	loadBots(workspaceId: string, force = false): Promise<void> {
		return this.run(
			`bots:${workspaceId}`,
			this.botsOf(workspaceId),
			force,
			(v) => (this.bots = { ...this.bots, [workspaceId]: v }),
			async () =>
				loadFromResult(await this.apis.bots(workspaceId), (items) => ({
					items: Array.isArray(items) ? items : [],
					total: null
				}))
		);
	}

	agentsOf(projectId: string): Load<AgentLike> {
		return this.agents[projectId] ?? IDLE;
	}
	loadAgents(projectId: string, force = false): Promise<void> {
		return this.run(
			`agents:${projectId}`,
			this.agentsOf(projectId),
			force,
			(v) => (this.agents = { ...this.agents, [projectId]: v }),
			async () => loadFromResult(await this.apis.aiAgents(projectId), fromPage)
		);
	}

	formsOf(projectId: string): Load<FormRef> {
		return this.forms[projectId] ?? IDLE;
	}
	loadForms(projectId: string, force = false): Promise<void> {
		return this.run(
			`forms:${projectId}`,
			this.formsOf(projectId),
			force,
			(v) => (this.forms = { ...this.forms, [projectId]: v }),
			async () =>
				loadFromResult(await this.apis.forms(projectId), (page) => {
					const { items, total } = fromPage(page);
					return { items: items.map((f) => ({ id: f.id, name: f.name })), total };
				})
		);
	}

	/** Apps count of a project: null until its discovery has loaded. */
	appsCountOf(projectId: string): number | null {
		return this.apps[projectId] ?? null;
	}
	setAppsCount(projectId: string, count: number | null): void {
		if (this.apps[projectId] === count) return;
		this.apps = { ...this.apps, [projectId]: count };
	}
}
