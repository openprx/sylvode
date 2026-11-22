/**
 * The answers of the bot connection API, read at the boundary.
 *
 * Nothing here asserts a type: every field is checked where it is read, and an answer that does
 * not have the documented shape reads as `null`, so a half-understood answer is a load failure
 * rather than a half-rendered section.
 */

export interface ProjectRef {
	id: string;
	name: string;
}

export interface Governance {
	max_domain_level: Level;
	can_veto_human_consensus: boolean;
	reason_min_length: number;
	domain_overrides: Record<string, Level>;
}

export const LEVELS = ['observer', 'advisor', 'voter', 'vetoer', 'autonomous'] as const;
export type Level = (typeof LEVELS)[number];
export const CLIS = ['claude', 'codex', 'opencode'] as const;
export type Cli = (typeof CLIS)[number];
export const CHANNELS = ['tmux', 'headless'] as const;
export type Channel = (typeof CHANNELS)[number];

export interface Connection {
	project_id: string;
	version: number;
	enabled: boolean;
	bot: { id: string; name: string; token_prefix: string; token_active: boolean };
	runner: {
		url: string;
		webhook_id: string;
		credentials_issued_at: string;
		shared_with_projects: ProjectRef[];
	};
	execution: {
		channel: Channel;
		cli: Cli;
		working_directory: string;
		/** `<PROJECT KEY>-<cli>` of a tmux connection; `null` for headless (or when not sent). */
		session_name: string | null;
		on_complete_state: string;
		on_fail_label_id: string | null;
		approval_gate: string;
	};
	governance: Governance;
}

export type RunnerState = 'waiting' | 'online' | 'not_reporting' | 'unreachable';
export type ComponentState = 'ready' | 'warning' | 'missing' | 'disabled' | 'unknown';
export type DeliveryState = 'delivered' | 'refused' | 'failed' | 'none';

export type SessionState = 'running' | 'absent' | 'cli_not_running' | 'ambiguous' | 'dialog';
export type SessionOrigin = 'runner' | 'operator';
export type SessionHooks = 'seen' | 'none';
export type SessionPermissions = 'bypass' | 'normal' | 'unknown';
export type SessionMcp = 'ok' | 'missing' | 'other' | 'stale';

/**
 * The runner's row for the tmux session of a connection (design 6.6). Every field but `status`
 * is absent when the runner did not report it.
 */
export interface SessionRow {
	session_name: string | null;
	status: 'ok' | 'warn' | 'fail' | null;
	state: SessionState | null;
	created_by: SessionOrigin | null;
	hooks: SessionHooks | null;
	permissions: SessionPermissions | null;
	mcp: SessionMcp | null;
}

export interface Status {
	runner: {
		state: RunnerState;
		reported_at: string | null;
		version: string | null;
		warning: string | null;
	};
	cli: { state: ComponentState; version: string | null };
	tmux: { state: ComponentState; version: string | null } | null;
	/** The session rows of 6.6; `null` when the status does not carry them (headless, no report). */
	tmux_session: SessionRow | null;
	/** Whether tasks of this connection run with permission checks off; `null` when unknown. */
	bypass: boolean | null;
	last_delivery: {
		state: DeliveryState;
		at: string | null;
		project_key: string | null;
		task_id: string | null;
		reason_code: string | null;
	};
}

/** The configuration block of a connect or rotate answer. Holds the token and signing secret. */
export interface BlockAnswer {
	text: string;
}

export function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(value: unknown): string | null {
	return typeof value === 'string' ? value : null;
}

function member<T extends string>(list: readonly T[], value: unknown): T | null {
	return typeof value === 'string' ? (list.find((item) => item === value) ?? null) : null;
}

function projects(value: unknown): ProjectRef[] | null {
	if (!Array.isArray(value)) return null;
	const out: ProjectRef[] = [];
	for (const item of value) {
		if (!isRecord(item)) return null;
		const id = str(item.id);
		const name = str(item.name);
		if (id === null || name === null) return null;
		out.push({ id, name });
	}
	return out;
}

function governance(value: unknown): Governance | null {
	if (!isRecord(value)) return null;
	const level = member(LEVELS, value.max_domain_level);
	const length = value.reason_min_length;
	const overrides = value.domain_overrides;
	if (
		level === null ||
		typeof value.can_veto_human_consensus !== 'boolean' ||
		typeof length !== 'number' ||
		!Number.isInteger(length) ||
		!isRecord(overrides)
	)
		return null;
	const domain_overrides: Record<string, Level> = {};
	for (const [key, entry] of Object.entries(overrides)) {
		const entryLevel = member(LEVELS, entry);
		if (entryLevel === null) return null;
		domain_overrides[key] = entryLevel;
	}
	return {
		max_domain_level: level,
		can_veto_human_consensus: value.can_veto_human_consensus,
		reason_min_length: length,
		domain_overrides
	};
}

/** A full connection (the view owners and admins get), or `null` when it is not one. */
export function parseConnection(value: unknown): Connection | null {
	if (!isRecord(value) || !isRecord(value.bot) || !isRecord(value.runner)) return null;
	if (!isRecord(value.execution)) return null;
	const { bot, runner, execution } = value;
	const shared = projects(runner.shared_with_projects);
	const channel = member(CHANNELS, execution.channel);
	const cli = member(CLIS, execution.cli);
	const gov = governance(value.governance);
	const projectId = str(value.project_id);
	const botId = str(bot.id);
	const botName = str(bot.name);
	const prefix = str(bot.token_prefix);
	const url = str(runner.url);
	const webhook = str(runner.webhook_id);
	const issued = str(runner.credentials_issued_at);
	const directory = str(execution.working_directory);
	const onComplete = str(execution.on_complete_state);
	const gate = str(execution.approval_gate);
	const label = execution.on_fail_label_id;
	// Optional: only a tmux connection has one.
	const sessionName = execution.session_name === undefined ? null : str(execution.session_name);
	if (
		projectId === null ||
		botId === null ||
		botName === null ||
		prefix === null ||
		url === null ||
		webhook === null ||
		issued === null ||
		directory === null ||
		onComplete === null ||
		gate === null ||
		shared === null ||
		channel === null ||
		cli === null ||
		gov === null ||
		typeof value.version !== 'number' ||
		!Number.isInteger(value.version) ||
		typeof value.enabled !== 'boolean' ||
		typeof bot.token_active !== 'boolean' ||
		!(label === null || typeof label === 'string')
	)
		return null;
	return {
		project_id: projectId,
		version: value.version,
		enabled: value.enabled,
		bot: { id: botId, name: botName, token_prefix: prefix, token_active: bot.token_active },
		runner: {
			url,
			webhook_id: webhook,
			credentials_issued_at: issued,
			shared_with_projects: shared
		},
		execution: {
			channel,
			cli,
			working_directory: directory,
			session_name: sessionName,
			on_complete_state: onComplete,
			on_fail_label_id: label,
			approval_gate: gate
		},
		governance: gov
	};
}

const RUNNER_STATES = ['waiting', 'online', 'not_reporting', 'unreachable'] as const;
const COMPONENT_STATES = ['ready', 'warning', 'missing', 'disabled', 'unknown'] as const;
const DELIVERY_STATES = ['delivered', 'refused', 'failed', 'none'] as const;

function component(value: unknown): { state: ComponentState; version: string | null } | null {
	if (!isRecord(value)) return null;
	const state = member(COMPONENT_STATES, value.state);
	return state === null ? null : { state, version: str(value.version) };
}

const SESSION_STATES = ['running', 'absent', 'cli_not_running', 'ambiguous', 'dialog'] as const;
const SESSION_ORIGINS = ['runner', 'operator'] as const;
const SESSION_HOOKS = ['seen', 'none'] as const;
const SESSION_PERMISSIONS = ['bypass', 'normal', 'unknown'] as const;
const SESSION_MCP = ['ok', 'missing', 'other', 'stale'] as const;
const ROW_STATUS = ['ok', 'warn', 'fail'] as const;

/**
 * The session row of a status. Tolerant on purpose: a missing or unrecognised row, or a value
 * outside its vocabulary, reads as absent and never fails the status.
 */
function sessionRow(value: unknown): SessionRow | null {
	if (!isRecord(value)) return null;
	return {
		session_name: str(value.session_name),
		status: member(ROW_STATUS, value.status),
		state: member(SESSION_STATES, value.state),
		created_by: member(SESSION_ORIGINS, value.created_by),
		hooks: member(SESSION_HOOKS, value.hooks),
		permissions: member(SESSION_PERMISSIONS, value.permissions),
		mcp: member(SESSION_MCP, value.mcp)
	};
}

/** The status of a connection, or `null` when the answer does not have the documented shape. */
export function parseStatus(value: unknown): Status | null {
	if (!isRecord(value) || !isRecord(value.runner) || !isRecord(value.last_delivery)) return null;
	const runnerState = member(RUNNER_STATES, value.runner.state);
	const deliveryState = member(DELIVERY_STATES, value.last_delivery.state);
	const cli = component(value.cli);
	if (runnerState === null || deliveryState === null || cli === null) return null;
	const tmux = value.tmux === null || value.tmux === undefined ? null : component(value.tmux);
	if (value.tmux !== null && value.tmux !== undefined && tmux === null) return null;
	return {
		runner: {
			state: runnerState,
			reported_at: str(value.runner.reported_at),
			version: str(value.runner.version),
			warning: str(value.runner.warning)
		},
		cli,
		tmux,
		tmux_session: sessionRow(value.tmux_session),
		bypass: typeof value.bypass === 'boolean' ? value.bypass : null,
		last_delivery: {
			state: deliveryState,
			at: str(value.last_delivery.at),
			project_key: str(value.last_delivery.project_key),
			task_id: str(value.last_delivery.task_id),
			reason_code: str(value.last_delivery.reason_code)
		}
	};
}

/** The block of a connect or rotate answer, `null` when there is none. */
export function parseBlock(value: unknown): BlockAnswer | null {
	if (!isRecord(value)) return null;
	const text = str(value.text);
	return text === null || text === '' ? null : { text };
}

/** One connection of the project with its status. */
export interface ConnectionItem {
	connection: Connection;
	status: Status | null;
}

/** What a reading answer holds: the full view, or the reduced member view. */
export type ReadAnswer =
	| { kind: 'full'; connection: Connection; status: Status | null }
	| { kind: 'reduced' };

function isReduced(value: unknown): boolean {
	return isRecord(value) && isRecord(value.bot) && !isRecord(value.runner);
}

/** The answer of one connection: `{connection, status}`. */
export function parseRead(value: unknown): ReadAnswer | null {
	if (!isRecord(value)) return null;
	const connection = parseConnection(value.connection);
	if (connection !== null) return { kind: 'full', connection, status: parseStatus(value.status) };
	return isReduced(value.connection) ? { kind: 'reduced' } : null;
}

/** What the list holds: every connection with its status, or the reduced member views. */
export type ListAnswer = { kind: 'full'; items: ConnectionItem[] } | { kind: 'reduced' };

/** The answer of the list: `{connections: [{connection, status}]}`, or member views. */
export function parseList(value: unknown): ListAnswer | null {
	if (!isRecord(value) || !Array.isArray(value.connections)) return null;
	const items: ConnectionItem[] = [];
	let reduced = false;
	for (const entry of value.connections) {
		const connection = isRecord(entry) ? parseConnection(entry.connection) : null;
		if (connection !== null && isRecord(entry)) {
			items.push({ connection, status: parseStatus(entry.status) });
			continue;
		}
		// Members get the reduced views, bare or wrapped.
		if (isReduced(entry) || (isRecord(entry) && isReduced(entry.connection))) {
			reduced = true;
			continue;
		}
		return null;
	}
	return reduced && items.length === 0 ? { kind: 'reduced' } : { kind: 'full', items };
}

/** A write's answer: the connection and its status, and the block when one was issued. */
export interface WriteAnswer {
	connection: Connection;
	status: Status | null;
	block: BlockAnswer | null;
}

export function parseWrite(value: unknown): WriteAnswer | null {
	if (!isRecord(value)) return null;
	const connection = parseConnection(value.connection);
	if (connection === null) return null;
	return { connection, status: parseStatus(value.status), block: parseBlock(value.config_block) };
}

export interface RemoveAnswer {
	still: ProjectRef[];
}

export function parseRemove(value: unknown): RemoveAnswer | null {
	if (!isRecord(value) || value.removed !== true) return null;
	const still = projects(value.bot_still_connected_to);
	return still === null ? null : { still };
}
