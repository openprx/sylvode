/**
 * What each destination's column lists (contract 2.4.2), derived from data the caller already
 * has. Pure: no requests, no stores.
 */
import { PERSONAL_SCOPE, type FlowAvailability, type ScopeKey } from '../destinations';
import type { RecentEntry } from '../storage';
import type { NavRow } from './rows';
import type { WorkspaceRef } from './scope';

export type Translate = (key: string, values?: Record<string, string | number>) => string;

const enc = encodeURIComponent;

// --- Home ---

export function homeRows(input: { unread: number; tr: Translate }): NavRow[] {
	const row = (id: string, key: string, href: string, glyph: NavRow['glyph']): NavRow => ({
		id,
		label: input.tr(key),
		href,
		glyph,
		level: 0,
		parentId: null,
		attrs: { 'data-home-row': id }
	});
	const notifications = row('notifications', 'shell.column.home.notifications', '/inbox', 'bell');
	if (input.unread > 0) notifications.count = input.unread;
	return [
		row('overview', 'shell.column.home.overview', '/workspace', 'house'),
		notifications,
		row('my-work', 'shell.column.home.myWork', '/my-work', 'list-checks')
	];
}

/** Up to eight recent task resources, most recent first. */
export function recentRows(recent: readonly RecentEntry[]): NavRow[] {
	return recent.slice(0, 8).map((entry, index) => ({
		id: `recent:${index}:${entry.href}`,
		label: entry.title,
		href: entry.href,
		glyph: 'clock' as const,
		level: 0,
		parentId: null,
		attrs: { 'data-home-row': 'recent' }
	}));
}

// --- Projects ---

export interface ProjectLike {
	id: string;
	name: string;
	workspace_id: string;
	workspace_name?: string;
	type_key: string;
}

/** The projects of one workspace, by name; the option store lists every workspace's. */
export function projectsOfWorkspace<T extends ProjectLike>(
	options: readonly T[],
	workspaceId: string,
	locale?: string
): T[] {
	const collator = new Intl.Collator(locale);
	return options
		.filter((p) => p.workspace_id === workspaceId)
		.sort((a, b) => collator.compare(a.name, b.name) || a.id.localeCompare(b.id));
}

/** Personal scope: one row per workspace; choosing one sets the scope. */
export function workspaceChoiceRows(workspaces: readonly WorkspaceRef[]): NavRow[] {
	return workspaces.map((w) => ({
		id: `workspace:${w.id}`,
		label: w.name,
		href: `/workspace/${enc(w.id)}/projects`,
		glyph: 'briefcase' as const,
		level: 0,
		parentId: null,
		attrs: { 'data-workspace-id': w.id }
	}));
}

/** Project ids that must be expanded because the URL is inside them (auto-expand on navigation). */
export function projectToExpand(
	ctx: { workspaceId: string | null; projectId: string | null },
	scope: ScopeKey,
	isExpanded: (projectId: string) => boolean
): string | null {
	if (scope === PERSONAL_SCOPE || !ctx.projectId || ctx.workspaceId !== scope) return null;
	return isExpanded(ctx.projectId) ? null : ctx.projectId;
}

/** True when the Forms group of the current project must open (a form is on screen). */
export function formsGroupToExpand(ctx: {
	projectId: string | null;
	formId: string | null;
}): string | null {
	return ctx.projectId && ctx.formId ? ctx.projectId : null;
}

/** Window focus refreshes the project list at most once per `minGapMs`. */
export const PROJECT_REFRESH_GAP_MS = 60_000;
export function shouldRefreshProjects(
	now: number,
	lastAt: number | null,
	minGapMs = PROJECT_REFRESH_GAP_MS
): boolean {
	return lastAt === null || now - lastAt >= minGapMs;
}

// --- Flow ---

export type FlowNotice =
	| { kind: 'unavailable-personal' }
	| { kind: 'off'; adminHref: string | null };

export interface FlowList {
	rows: NavRow[];
	/** Neutral line when a scope has no Flow. */
	notice: FlowNotice | null;
	/** The workspace flag is not known yet. */
	loading: boolean;
}

/**
 * Personal Flow ("Private") when available, the workspace's Flow when its flag is on. While the
 * flag is unknown, `loading` is true and no workspace row shows; when it is off a neutral notice
 * explains (admins also get the settings link).
 */
export function flowList(input: {
	scope: ScopeKey;
	workspaceName: string;
	flow: FlowAvailability;
	workspaceAdmin: boolean;
	tr: Translate;
}): FlowList {
	const { scope, flow, tr } = input;
	const rows: NavRow[] = [];
	let notice: FlowNotice | null = null;
	let loading = false;
	if (flow.personal)
		rows.push({
			id: 'flow:private',
			label: tr('shell.column.flow.private'),
			href: '/flow',
			glyph: 'workflow',
			level: 0,
			parentId: null,
			attrs: { 'data-flow-section': 'private', 'data-testid': 'global-flow-link' }
		});
	if (scope === PERSONAL_SCOPE) {
		if (!flow.personal) notice = { kind: 'unavailable-personal' };
		return { rows, notice, loading };
	}
	const enabled = flow.workspaces[scope];
	if (enabled === true)
		rows.push({
			id: `flow:workspace:${scope}`,
			label: input.workspaceName,
			href: `/workspace/${enc(scope)}/flow`,
			glyph: 'workflow',
			level: 0,
			parentId: null,
			attrs: { 'data-flow-section': 'workspace', 'data-workspace-id': scope }
		});
	else if (enabled === false)
		notice = {
			kind: 'off',
			adminHref: input.workspaceAdmin ? `/workspace/${enc(scope)}/settings/flow` : null
		};
	else loading = true;
	return { rows, notice, loading };
}

// --- Agents ---

export type AgentsMode = 'personal' | 'workspace';

export function agentsMode(scope: ScopeKey): AgentsMode {
	return scope === PERSONAL_SCOPE ? 'personal' : 'workspace';
}

/** Bots are listed only for owners and admins: the endpoint refuses everyone else. */
export function showBots(scope: ScopeKey, workspaceAdmin: boolean): boolean {
	return scope !== PERSONAL_SCOPE && workspaceAdmin;
}

export interface BotLike {
	id: string;
	name: string;
}
export interface AgentLike {
	id: string;
	name: string;
}

/** Bots link to the members page until bots move to their own settings section. */
export function botRows(workspaceId: string, bots: readonly BotLike[]): NavRow[] {
	return bots.map((bot) => ({
		id: `bot:${bot.id}`,
		label: bot.name,
		href: `/workspace/${enc(workspaceId)}/members`,
		glyph: 'bot' as const,
		level: 0,
		parentId: null,
		attrs: { 'data-agent-kind': 'bot', 'data-bot-id': bot.id }
	}));
}

export function projectAgentsHref(workspaceId: string, projectId: string): string {
	return `/workspace/${enc(workspaceId)}/projects/${enc(projectId)}/ai-agents`;
}

/**
 * AI participants: one row per project of the workspace; an expanded project lists its
 * participants as children (each links to the project's agents page).
 */
export function participantRows(input: {
	workspaceId: string;
	projects: readonly ProjectLike[];
	expanded: ReadonlySet<string>;
	agentsOf: (projectId: string) => readonly AgentLike[] | null;
}): NavRow[] {
	const rows: NavRow[] = [];
	for (const project of input.projects) {
		const href = projectAgentsHref(input.workspaceId, project.id);
		const open = input.expanded.has(project.id);
		rows.push({
			id: `ai-project:${project.id}`,
			label: project.name,
			href,
			glyph: 'folder',
			level: 0,
			parentId: null,
			expandable: true,
			expanded: open,
			attrs: { 'data-agent-kind': 'project', 'data-project-id': project.id }
		});
		if (!open) continue;
		for (const agent of input.agentsOf(project.id) ?? [])
			rows.push({
				id: `ai-agent:${project.id}:${agent.id}`,
				label: agent.name,
				href,
				glyph: 'sparkles',
				level: 1,
				parentId: `ai-project:${project.id}`,
				selectable: false,
				attrs: { 'data-agent-kind': 'ai', 'data-project-id': project.id, 'data-agent-id': agent.id }
			});
	}
	return rows;
}

// --- Governance ---

export function governanceRows(input: { instanceAdmin: boolean; tr: Translate }): NavRow[] {
	const items: Array<[string, string, string]> = [
		['centre', 'shell.column.governance.centre', '/governance'],
		['proposals', 'shell.column.governance.proposals', '/proposals'],
		['templates', 'shell.column.governance.templates', '/proposal-templates'],
		['impact', 'shell.column.governance.impact', '/impact-reviews'],
		['decisions', 'shell.column.governance.decisions', '/decisions/analytics'],
		['trust', 'shell.column.governance.trust', '/trust-board'],
		['appeals', 'shell.column.governance.appeals', '/trust-scores/appeals']
	];
	if (input.instanceAdmin)
		items.push(
			['config', 'shell.column.governance.config', '/governance/config'],
			['audit', 'shell.column.governance.audit', '/governance/audit-logs']
		);
	return items.map(([id, key, href]) => ({
		id: `governance:${id}`,
		label: input.tr(key),
		href,
		glyph: 'landmark' as const,
		level: 0,
		parentId: null,
		attrs: { 'data-governance-row': id }
	}));
}
