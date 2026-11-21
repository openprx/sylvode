/**
 * What the connection forms choose from: the workspace's already connected bots, the project's
 * workflow states, the workspace's labels and the project's decision domains. Each loader narrows
 * what it reads and answers an empty list when the list cannot be loaded, so a failing list
 * disables its own field instead of the whole section.
 */
import { botsApi } from '#lib/api/bots.js';
import { apiClient } from '#lib/api/client.js';
import { botConnectionApi } from '#lib/api/bot-connection.js';
import { labelsApi } from '#lib/api/labels.js';
import { workflowsApi } from '#lib/api/workflows.js';
import { parseManagedBy } from './managed';
import { isRecord } from './wire';

export interface ConnectedBotOption {
	id: string;
	name: string;
	runnerUrl: string;
	projects: number;
}

export interface Choice {
	value: string;
	label: string;
}

/**
 * Bots a connection made that serve a project other than `projectId`: the only bots "Connected
 * bot" offers. A bot made on the Bots page is neither offered nor accepted by the server. The
 * runner address of each is read from the connection of one of its projects.
 */
export async function loadConnectedBots(
	workspaceId: string,
	projectId: string
): Promise<ConnectedBotOption[]> {
	const bots = await botsApi.list(workspaceId);
	if (bots.code !== 0 || !Array.isArray(bots.data)) return [];
	const options: ConnectedBotOption[] = [];
	for (const bot of bots.data) {
		const managed = parseManagedBy(bot.managed_by);
		if (managed === null || managed.projects.length === 0) continue;
		if (managed.projects.some((project) => project.id === projectId)) continue;
		const first = managed.projects[0];
		if (first === undefined) continue;
		const read = await botConnectionApi.get(first.id);
		if (!read.ok || read.value.kind !== 'full') continue;
		options.push({
			id: bot.id,
			name: bot.name,
			runnerUrl: read.value.connection.runner.url,
			projects: managed.projects.length
		});
	}
	return options;
}

export async function loadStates(projectId: string): Promise<Choice[]> {
	const response = await workflowsApi.getEffectiveByProject(projectId);
	if (response.code !== 0 || !response.data || !Array.isArray(response.data.states)) return [];
	return response.data.states
		.filter((state) => typeof state.key === 'string')
		.map((state) => ({ value: state.key, label: state.display_name || state.key }));
}

export async function loadLabels(workspaceId: string): Promise<Choice[]> {
	const response = await labelsApi.list(workspaceId);
	if (response.code !== 0 || !Array.isArray(response.data?.items)) return [];
	return response.data.items.map((label) => ({ value: label.id, label: label.name }));
}

/**
 * The key an override is stored under for a domain: `normalize_domain_key` of its name. The
 * API's domain id is `<first 8 hex of the project id>-<key>`, so the key is read from it.
 */
export function domainKey(projectId: string, id: string, name: string): string {
	const prefix = `${projectId.replace(/-/g, '').slice(0, 8)}-`;
	if (id.startsWith(prefix) && id.length > prefix.length) return id.slice(prefix.length);
	return name
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9_-]/g, '_');
}

/** The project's decision domains, except `governance`, where an AI is always an observer. */
export async function loadDomains(projectId: string): Promise<Choice[]> {
	const response = await apiClient.get<unknown>(
		`/api/v1/projects/${encodeURIComponent(projectId)}/domains`
	);
	if (response.code !== 0 || !isRecord(response.data) || !Array.isArray(response.data.items))
		return [];
	const choices: Choice[] = [];
	for (const item of response.data.items) {
		if (!isRecord(item) || typeof item.id !== 'string' || typeof item.name !== 'string') continue;
		const key = domainKey(projectId, item.id, item.name);
		if (key !== 'governance') choices.push({ value: key, label: item.name });
	}
	return choices;
}
