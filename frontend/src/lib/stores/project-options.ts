import { get, writable } from 'svelte/store';
import { projectsApi, type Project } from '#lib/api/projects.js';
import { workspacesApi } from '#lib/api/workspaces.js';

export interface ProjectOption {
	id: string;
	name: string;
	workspace_id: string;
	workspace_name: string;
	type_key: string;
}

interface ProjectOptionsState {
	items: ProjectOption[];
	loading: boolean;
	loaded: boolean;
	error: string | null;
}

const initialState: ProjectOptionsState = {
	items: [],
	loading: false,
	loaded: false,
	error: null
};

export function createProjectOptionsStore() {
	const { subscribe, set, update } = writable<ProjectOptionsState>(initialState);
	let inFlight: Promise<ProjectOption[]> | null = null;
	let generation = 0;
	let userId: string | null = null;

	async function fetchProjectOptions(version: number): Promise<ProjectOption[]> {
		const wsRes = await workspacesApi.list();
		if (wsRes.code !== 0 || !wsRes.data) {
			return [];
		}

		if (version !== generation) return [];
		const workspaceItems = wsRes.data.items ?? [];
		const projectResults = await Promise.all(
			workspaceItems.map(async (workspace) => {
				const projectRes = await projectsApi.list(workspace.id, { page: 1, per_page: 100 });
				if (projectRes.code !== 0 || !projectRes.data) {
					return [] as ProjectOption[];
				}
				return (projectRes.data.items ?? []).map((project: Project) => ({
					id: project.id,
					name: project.name,
					workspace_id: workspace.id,
					workspace_name: workspace.name,
					type_key: project.type_key
				}));
			})
		);

		return projectResults
			.flat()
			.sort((a, b) =>
				`${a.workspace_name}/${a.name}`.localeCompare(`${b.workspace_name}/${b.name}`)
			);
	}

	async function ensureLoaded(force = false): Promise<ProjectOption[]> {
		const current = get({ subscribe });
		if (!force && current.loaded) {
			return current.items;
		}
		if (!force && inFlight) {
			return inFlight;
		}

		if (force) generation++;
		const version = generation;
		update((state) => ({ ...state, loading: true, error: null }));
		const task = (async () => {
			try {
				const items = await fetchProjectOptions(version);
				if (version !== generation) return [];
				update(() => ({ items, loading: false, loaded: true, error: null }));
				return items;
			} catch (error) {
				if (version !== generation) return [];
				const message = error instanceof Error ? error.message : 'Failed to load project options';
				update((state) => ({ ...state, loading: false, error: message }));
				return current.items;
			} finally {
				if (version === generation) inFlight = null;
			}
		})();

		inFlight = task;
		return task;
	}

	return {
		subscribe,
		ensureLoaded,
		refresh: () => ensureLoaded(true),
		reset: () => {
			generation++;
			inFlight = null;
			set(initialState);
		},
		selectUser: (id: string | null) => {
			if (id === userId) return;
			userId = id;
			generation++;
			inFlight = null;
			set(initialState);
		}
	};
}

export const projectOptionsStore = createProjectOptionsStore();
