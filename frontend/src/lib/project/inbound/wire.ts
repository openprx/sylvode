/**
 * The answers of the inbound source and alert rule management API, read at the boundary.
 *
 * Nothing here asserts a type: every field is checked where it is read, and an answer that does
 * not have the documented shape reads as `null`, so a half-understood answer is a load failure
 * rather than a half-rendered section. Codes the server may add later (outcomes, reasons, queue
 * causes, verdicts) stay plain strings; `status.ts` turns known ones into words.
 */
import { isRecord } from '#lib/project/connection/wire.js';

export { isRecord };

export const SEVERITIES = ['info', 'low', 'medium', 'high', 'critical'] as const;
export type Severity = (typeof SEVERITIES)[number];

export const RULE_ACTIONS = ['work_item', 'work_item_and_task'] as const;
export type RuleAction = (typeof RULE_ACTIONS)[number];

export const PRIORITIES = ['low', 'medium', 'high', 'urgent'] as const;
export type Priority = (typeof PRIORITIES)[number];

/** Severity word to work item priority, for every severity (the rule's `priority_map`). */
export type PriorityMap = Record<Severity, Priority>;

/** The only warning code the rule write answers carry today. */
export const WARNING_BOT_NOT_CONNECTED = 'bot_not_connected_to_project';

// --- small readers --------------------------------------------------------------------------

function str(value: unknown): string | null {
	return typeof value === 'string' ? value : null;
}

function strOrNull(value: unknown): string | null | undefined {
	if (value === null) return null;
	return typeof value === 'string' ? value : undefined;
}

function int(value: unknown): number | null {
	return typeof value === 'number' && Number.isInteger(value) ? value : null;
}

function strings(value: unknown): string[] | null {
	if (!Array.isArray(value)) return null;
	const out: string[] = [];
	for (const item of value) {
		if (typeof item !== 'string') return null;
		out.push(item);
	}
	return out;
}

function member<T extends string>(list: readonly T[], value: unknown): T | null {
	return typeof value === 'string' ? (list.find((item) => item === value) ?? null) : null;
}

// --- sources --------------------------------------------------------------------------------

export interface Source {
	id: string;
	workspace_id: string;
	project_id: string;
	name: string;
	token_prefix: string;
	require_signature: boolean;
	has_signing_secret: boolean;
	ip_allowlist: string[];
	acting_user_id: string | null;
	enabled: boolean;
	rate_per_minute: number;
	daily_cap: number;
	last_used_at: string | null;
	last_used_ip: string | null;
	created_at: string;
	updated_at: string;
	/** The server's verdict that the acting user is no longer an owner or admin of the workspace. */
	suspended: boolean;
	/** The acting user's name; `null` when the user was removed. */
	acting_user_name: string | null;
}

export function parseSource(value: unknown): Source | null {
	if (!isRecord(value)) return null;
	const allowlist = strings(value.ip_allowlist);
	const acting = strOrNull(value.acting_user_id);
	const lastAt = strOrNull(value.last_used_at);
	const lastIp = strOrNull(value.last_used_ip);
	const id = str(value.id);
	const workspace = str(value.workspace_id);
	const project = str(value.project_id);
	const name = str(value.name);
	const prefix = str(value.token_prefix);
	const rate = int(value.rate_per_minute);
	const cap = int(value.daily_cap);
	const created = str(value.created_at);
	const updated = str(value.updated_at);
	const actingName = strOrNull(value.acting_user_name);
	if (
		id === null ||
		workspace === null ||
		project === null ||
		name === null ||
		prefix === null ||
		allowlist === null ||
		acting === undefined ||
		lastAt === undefined ||
		lastIp === undefined ||
		rate === null ||
		cap === null ||
		created === null ||
		updated === null ||
		actingName === undefined ||
		typeof value.suspended !== 'boolean' ||
		typeof value.require_signature !== 'boolean' ||
		typeof value.has_signing_secret !== 'boolean' ||
		typeof value.enabled !== 'boolean'
	)
		return null;
	return {
		id,
		workspace_id: workspace,
		project_id: project,
		name,
		token_prefix: prefix,
		require_signature: value.require_signature,
		has_signing_secret: value.has_signing_secret,
		ip_allowlist: allowlist,
		acting_user_id: acting,
		enabled: value.enabled,
		rate_per_minute: rate,
		daily_cap: cap,
		last_used_at: lastAt,
		last_used_ip: lastIp,
		created_at: created,
		updated_at: updated,
		suspended: value.suspended,
		acting_user_name: actingName
	};
}

/** The sources of a project with the server-wide switch and the public address. */
export interface SourceList {
	sources: Source[];
	/** False when `[inbound] enabled = false`: every event is refused until it is switched on. */
	inbound_enabled: boolean;
	/** The address events are posted to; `null` when the server cannot tell. */
	endpoint_url: string | null;
}

export function parseSourceList(value: unknown): SourceList | null {
	if (!isRecord(value) || !Array.isArray(value.sources)) return null;
	const endpoint = strOrNull(value.endpoint_url);
	if (typeof value.inbound_enabled !== 'boolean' || endpoint === undefined) return null;
	const sources: Source[] = [];
	for (const item of value.sources) {
		const source = parseSource(item);
		if (source === null) return null;
		sources.push(source);
	}
	return { sources, inbound_enabled: value.inbound_enabled, endpoint_url: endpoint };
}

/**
 * The answer of a source create or rotate: the only place the token and the signing secret ever
 * appear. The caller puts them into the one-time component state and nowhere else.
 */
export interface SecretsAnswer {
	source: Source;
	token: string;
	signing_secret: string;
	endpoint_url: string | null;
}

export function parseSecretsAnswer(value: unknown): SecretsAnswer | null {
	if (!isRecord(value)) return null;
	const source = parseSource(value.source);
	const token = str(value.token);
	const secret = str(value.signing_secret);
	const endpoint = strOrNull(value.endpoint_url);
	if (source === null || !token || !secret || endpoint === undefined) return null;
	return { source, token, signing_secret: secret, endpoint_url: endpoint };
}

// --- events ---------------------------------------------------------------------------------

export interface WorkItemRef {
	id: string;
	identifier: string;
}

/**
 * A stored event. `title`, `summary`, `subject` and `payload` were written by an external system
 * and may be hostile: they are shown as text only, never as markup.
 */
export interface InboundEvent {
	id: string;
	event_id: string;
	kind: string;
	severity_rank: number;
	dedup_key: string;
	occurred_at: string;
	received_at: string;
	title: string;
	summary: string;
	subject: unknown;
	payload: unknown;
	signed: boolean;
	client_ip: string | null;
	outcome: string;
	rule_id: string | null;
	alert_id: string | null;
	task_id: string | null;
	/** The work item the event's alert belongs to; `null` when it led to none. */
	work_item: WorkItemRef | null;
}

function workItem(value: unknown): WorkItemRef | null | undefined {
	if (value === null) return null;
	if (!isRecord(value)) return undefined;
	const id = str(value.id);
	const identifier = str(value.identifier);
	return id === null || identifier === null ? undefined : { id, identifier };
}

export function parseEvent(value: unknown): InboundEvent | null {
	if (!isRecord(value)) return null;
	const clientIp = strOrNull(value.client_ip);
	const ruleId = strOrNull(value.rule_id);
	const alertId = strOrNull(value.alert_id);
	const taskId = strOrNull(value.task_id);
	const item = workItem(value.work_item);
	const id = str(value.id);
	const eventId = str(value.event_id);
	const kind = str(value.kind);
	const rank = int(value.severity_rank);
	const dedup = str(value.dedup_key);
	const occurred = str(value.occurred_at);
	const received = str(value.received_at);
	const title = str(value.title);
	const summary = str(value.summary);
	const outcome = str(value.outcome);
	if (
		id === null ||
		eventId === null ||
		kind === null ||
		rank === null ||
		dedup === null ||
		occurred === null ||
		received === null ||
		title === null ||
		summary === null ||
		outcome === null ||
		clientIp === undefined ||
		ruleId === undefined ||
		alertId === undefined ||
		taskId === undefined ||
		item === undefined ||
		typeof value.signed !== 'boolean'
	)
		return null;
	return {
		id,
		event_id: eventId,
		kind,
		severity_rank: rank,
		dedup_key: dedup,
		occurred_at: occurred,
		received_at: received,
		title,
		summary,
		subject: value.subject ?? null,
		payload: value.payload ?? null,
		signed: value.signed,
		client_ip: clientIp,
		outcome,
		rule_id: ruleId,
		alert_id: alertId,
		task_id: taskId,
		work_item: item
	};
}

export function parseEvents(value: unknown): InboundEvent[] | null {
	if (!Array.isArray(value)) return null;
	const out: InboundEvent[] = [];
	for (const item of value) {
		const event = parseEvent(item);
		if (event === null) return null;
		out.push(event);
	}
	return out;
}

// --- rules ----------------------------------------------------------------------------------

export interface Rule {
	id: string;
	workspace_id: string;
	project_id: string;
	name: string;
	position: number;
	enabled: boolean;
	source_id: string | null;
	kind_patterns: string[];
	min_severity: Severity;
	action: RuleAction;
	bot_user_id: string | null;
	instructions: string;
	priority_map: PriorityMap;
	label_ids: string[];
	notify_user_ids: string[];
	max_tasks_per_hour: number;
	max_active_tasks: number;
	dedup_window_secs: number;
	rearm_after_secs: number;
	task_max_attempts: number;
	version: number;
}

function priorityMap(value: unknown): PriorityMap | null {
	if (!isRecord(value)) return null;
	const out = {} as PriorityMap;
	for (const severity of SEVERITIES) {
		const priority = member(PRIORITIES, value[severity]);
		if (priority === null) return null;
		out[severity] = priority;
	}
	return out;
}

export function parseRule(value: unknown): Rule | null {
	if (!isRecord(value)) return null;
	const source = strOrNull(value.source_id);
	const bot = strOrNull(value.bot_user_id);
	const patterns = strings(value.kind_patterns);
	const labels = strings(value.label_ids);
	const notify = strings(value.notify_user_ids);
	const min = member(SEVERITIES, value.min_severity);
	const action = member(RULE_ACTIONS, value.action);
	const map = priorityMap(value.priority_map);
	const id = str(value.id);
	const workspace = str(value.workspace_id);
	const project = str(value.project_id);
	const name = str(value.name);
	const instructions = str(value.instructions);
	const position = int(value.position);
	const perHour = int(value.max_tasks_per_hour);
	const active = int(value.max_active_tasks);
	const dedup = int(value.dedup_window_secs);
	const rearm = int(value.rearm_after_secs);
	const attempts = int(value.task_max_attempts);
	const version = int(value.version);
	if (
		id === null ||
		workspace === null ||
		project === null ||
		name === null ||
		instructions === null ||
		position === null ||
		perHour === null ||
		active === null ||
		dedup === null ||
		rearm === null ||
		attempts === null ||
		version === null ||
		source === undefined ||
		bot === undefined ||
		patterns === null ||
		labels === null ||
		notify === null ||
		min === null ||
		action === null ||
		map === null ||
		typeof value.enabled !== 'boolean'
	)
		return null;
	return {
		id,
		workspace_id: workspace,
		project_id: project,
		name,
		position,
		enabled: value.enabled,
		source_id: source,
		kind_patterns: patterns,
		min_severity: min,
		action,
		bot_user_id: bot,
		instructions,
		priority_map: map,
		label_ids: labels,
		notify_user_ids: notify,
		max_tasks_per_hour: perHour,
		max_active_tasks: active,
		dedup_window_secs: dedup,
		rearm_after_secs: rearm,
		task_max_attempts: attempts,
		version
	};
}

export function parseRules(value: unknown): Rule[] | null {
	if (!Array.isArray(value)) return null;
	const out: Rule[] = [];
	for (const item of value) {
		const rule = parseRule(item);
		if (rule === null) return null;
		out.push(rule);
	}
	return out;
}

/** A rule create or replace answer: the rule and the warning codes to show on its row. */
export interface RuleWrite {
	rule: Rule;
	warnings: string[];
}

export function parseRuleWrite(value: unknown): RuleWrite | null {
	if (!isRecord(value)) return null;
	const rule = parseRule(value.rule);
	const warnings = strings(value.warnings);
	return rule === null || warnings === null ? null : { rule, warnings };
}

/** A delete answer carries nothing the caller needs; a non-object is still refused. */
export function parseDeleted(value: unknown): { deleted: true } | null {
	return isRecord(value) && value.deleted === true ? { deleted: true } : null;
}

// --- rule test ------------------------------------------------------------------------------

export interface RuleVerdict {
	id: string;
	name: string;
	position: number;
	matched: boolean;
	/** `matched`, or why the rule does not handle the event (a code; may be new). */
	reason: string;
	/**
	 * This verdict is the unsaved draft rule. A draft that replaces nothing has the nil UUID as its
	 * `id` (`isSavedRuleId`); a draft that replaces a rule carries that rule's id.
	 */
	draft: boolean;
}

export interface TestCaps {
	tasks_last_hour: number;
	active_for_rule: number;
	active_in_workspace: number;
	max_tasks_per_hour: number;
	max_active_tasks: number;
	max_active_tasks_per_workspace: number;
	alerts_opened_last_hour: number;
	max_alerts_per_rule_per_hour: number;
}

export interface TestResult {
	matched: boolean;
	rule: RuleVerdict | null;
	rules_evaluated: RuleVerdict[];
	alert: string;
	task: string;
	queue_cause: string | null;
	would_escalate: boolean;
	caps: TestCaps | null;
	notify_recipients: number;
	title: string;
}

/** The id the server gives a draft rule that replaces no saved rule: nothing is stored under it. */
export const NIL_UUID = '00000000-0000-0000-0000-000000000000';

/** Whether a verdict's `id` names a rule that is stored; the nil id of a new draft does not. */
export function isSavedRuleId(id: string): boolean {
	return id !== NIL_UUID;
}

function parseVerdict(value: unknown): RuleVerdict | null {
	if (!isRecord(value)) return null;
	const id = str(value.id);
	const name = str(value.name);
	const position = int(value.position);
	const reason = str(value.reason);
	if (
		id === null ||
		name === null ||
		position === null ||
		reason === null ||
		typeof value.matched !== 'boolean'
	)
		return null;
	if (typeof value.draft !== 'boolean') return null;
	return { id, name, position, matched: value.matched, reason, draft: value.draft };
}

function parseCaps(value: unknown): TestCaps | null {
	if (!isRecord(value)) return null;
	const fields = {
		tasks_last_hour: int(value.tasks_last_hour),
		active_for_rule: int(value.active_for_rule),
		active_in_workspace: int(value.active_in_workspace),
		max_tasks_per_hour: int(value.max_tasks_per_hour),
		max_active_tasks: int(value.max_active_tasks),
		max_active_tasks_per_workspace: int(value.max_active_tasks_per_workspace),
		alerts_opened_last_hour: int(value.alerts_opened_last_hour),
		max_alerts_per_rule_per_hour: int(value.max_alerts_per_rule_per_hour)
	};
	return Object.values(fields).some((n) => n === null) ? null : (fields as TestCaps);
}

export function parseTestResult(value: unknown): TestResult | null {
	if (!isRecord(value) || !Array.isArray(value.rules_evaluated)) return null;
	const rule = value.rule === null ? null : parseVerdict(value.rule);
	if (value.rule !== null && rule === null) return null;
	const evaluated: RuleVerdict[] = [];
	for (const item of value.rules_evaluated) {
		const verdict = parseVerdict(item);
		if (verdict === null) return null;
		evaluated.push(verdict);
	}
	const caps = value.caps === null || value.caps === undefined ? null : parseCaps(value.caps);
	if (value.caps !== null && value.caps !== undefined && caps === null) return null;
	const alert = str(value.alert);
	const task = str(value.task);
	const cause = strOrNull(value.queue_cause);
	const recipients = int(value.notify_recipients);
	const title = str(value.title);
	if (
		alert === null ||
		task === null ||
		cause === undefined ||
		recipients === null ||
		title === null ||
		typeof value.matched !== 'boolean' ||
		typeof value.would_escalate !== 'boolean'
	)
		return null;
	return {
		matched: value.matched,
		rule,
		rules_evaluated: evaluated,
		alert,
		task,
		queue_cause: cause,
		would_escalate: value.would_escalate,
		caps,
		notify_recipients: recipients,
		title
	};
}

// --- the alert of a work item ---------------------------------------------------------------

/**
 * The read-only alert facts of a work item, as `GET /api/v1/issues/{id}/alert` returns them
 * (`apps/api/src/inbound/alert_view.rs`). `source`, `kind`, `rule` and every name in them were
 * written by an external system or an administrator and are shown as plain text only.
 */
export interface IssueAlert {
	issue_id: string;
	alert_id: string;
	source: { id: string; name: string } | null;
	kind: string | null;
	/** `info` .. `critical`; null when the rank is unknown to the server. */
	severity: string | null;
	occurrences: number;
	first_seen_at: string;
	last_seen_at: string;
	open: boolean;
	rule: { id: string; name: string } | null;
	task: { id: string; status: string } | null;
	/** `none`, `created`, `waiting`, `suppressed`; anything else is kept as sent. */
	task_state: string;
	queued_at: string | null;
	queue_expires_at: string | null;
}

/** Absent or null is `null`; any other type is malformed (`undefined`). */
function absentOrStr(raw: Record<string, unknown>, key: string): string | null | undefined {
	const value = raw[key];
	if (value === undefined || value === null) return null;
	return typeof value === 'string' ? value : undefined;
}

function idName(value: unknown): { id: string; name: string } | null | undefined {
	if (value === undefined || value === null) return null;
	if (!isRecord(value)) return undefined;
	const id = str(value.id);
	const name = str(value.name);
	return id === null || name === null ? undefined : { id, name };
}

export function parseIssueAlert(value: unknown): IssueAlert | null {
	if (!isRecord(value)) return null;
	const issueId = str(value.issue_id);
	const alertId = str(value.alert_id);
	const firstSeen = str(value.first_seen_at);
	const lastSeen = str(value.last_seen_at);
	const taskState = str(value.task_state);
	const occurrences = value.occurrences;
	if (
		issueId === null ||
		alertId === null ||
		firstSeen === null ||
		lastSeen === null ||
		taskState === null ||
		typeof occurrences !== 'number' ||
		!Number.isFinite(occurrences) ||
		typeof value.open !== 'boolean'
	) {
		return null;
	}
	const source = idName(value.source);
	const rule = idName(value.rule);
	const kind = absentOrStr(value, 'kind');
	const severity = absentOrStr(value, 'severity');
	const queuedAt = absentOrStr(value, 'queued_at');
	const expiresAt = absentOrStr(value, 'queue_expires_at');
	if (
		source === undefined ||
		rule === undefined ||
		kind === undefined ||
		severity === undefined ||
		queuedAt === undefined ||
		expiresAt === undefined
	) {
		return null;
	}
	let task: IssueAlert['task'] = null;
	if (value.task !== undefined && value.task !== null) {
		if (!isRecord(value.task)) return null;
		const id = str(value.task.id);
		const status = str(value.task.status);
		if (id === null || status === null) return null;
		task = { id, status };
	}
	return {
		issue_id: issueId,
		alert_id: alertId,
		source,
		kind,
		severity,
		occurrences,
		first_seen_at: firstSeen,
		last_seen_at: lastSeen,
		open: value.open,
		rule,
		task,
		task_state: taskState,
		queued_at: queuedAt,
		queue_expires_at: expiresAt
	};
}
