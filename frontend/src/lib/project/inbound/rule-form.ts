/**
 * The alert rule form as pure logic: drafts, defaults, validation, the request bodies and the
 * reorder step. No requests, no stores, no DOM.
 *
 * Saving a rule REPLACES the whole rule on the server and any field left out takes its default
 * (a missing recipient list becomes just the saver). So every body built here, from a draft or
 * from a stored rule (enable switch), states every field of `RuleBody`; `RULE_BODY_FIELDS` is the
 * list the tests hold the builders to.
 */
import type { PriorityBody, ReplaceRuleBody, RuleBody } from '#lib/api/inbound.js';
import { SEVERITIES, type Priority, type Rule, type RuleAction, type Severity } from './wire.js';

/** Every field a rule body states. `bot_user_id` only with `work_item_and_task`. */
export const RULE_BODY_FIELDS = [
	'name',
	'enabled',
	'source_id',
	'kind_patterns',
	'min_severity',
	'action',
	'instructions',
	'priority_map',
	'label_ids',
	'notify_user_ids',
	'max_tasks_per_hour',
	'max_active_tasks',
	'dedup_window_secs',
	'rearm_after_secs',
	'task_max_attempts'
] as const;

// --- limits and defaults --------------------------------------------------------------------

export const NAME_MAX = 100;
export const KINDS_MAX = 32;
export const INSTRUCTIONS_MAX = 4000;
export const RECIPIENTS_MAX = 50;
export const LABELS_MAX = 50;

export const TASKS_PER_HOUR = { min: 1, max: 120, default: 6 } as const;
export const ACTIVE_TASKS = { min: 1, max: 20, default: 2 } as const;
export const ATTEMPTS = { min: 1, max: 10, default: 3 } as const;
/** 60 seconds to 7 days. */
export const DEDUP_WINDOW = { min: 60, max: 7 * 86400, default: 3600 } as const;
/** 0 to 1 day. */
export const REARM_AFTER = { min: 0, max: 86400, default: 900 } as const;

/** Critical, high, medium, low, info. */
export const DEFAULT_PRIORITIES: PriorityBody = {
	critical: 'urgent',
	high: 'high',
	medium: 'medium',
	low: 'low',
	info: 'low'
};

export const DEFAULT_MIN_SEVERITY: Severity = 'high';

/** Decision D4: hand to a bot from High up, open a work item only below. */
export function defaultActionFor(severity: Severity): RuleAction {
	return severity === 'high' || severity === 'critical' ? 'work_item_and_task' : 'work_item';
}

// --- the draft ------------------------------------------------------------------------------

export interface RuleDraft {
	name: string;
	enabled: boolean;
	sourceId: string | null;
	/** One pattern per line, as typed. */
	kindsText: string;
	minSeverity: Severity;
	action: RuleAction;
	/** True once the person chose the action (or the rule is a stored one): it stops following the severity. */
	actionTouched: boolean;
	botId: string | null;
	instructions: string;
	notifyIds: string[];
	labelIds: string[];
	/** The number boxes keep their text, so a half-typed value is not rewritten. */
	tasksPerHour: string;
	activeTasks: string;
	attempts: string;
	/** Seconds; `null` while the amount of the duration field is not a whole number. */
	dedupWindowSecs: number | null;
	rearmAfterSecs: number | null;
	priorityMap: PriorityBody;
}

export interface NewDraftContext {
	/** The person writing the rule: the default recipient. */
	currentUserId: string | null;
	/** The project's connected bot when it has exactly one: the default bot. */
	connectedBotId: string | null;
}

/** The default bot of a new rule: the project's one enabled connection's bot, else none. */
export function onlyConnectedBot(connectedBotIds: ReadonlySet<string>): string | null {
	return connectedBotIds.size === 1 ? ([...connectedBotIds][0] ?? null) : null;
}

export function newDraft({ currentUserId, connectedBotId }: NewDraftContext): RuleDraft {
	return {
		name: '',
		enabled: true,
		sourceId: null,
		kindsText: '',
		minSeverity: DEFAULT_MIN_SEVERITY,
		action: defaultActionFor(DEFAULT_MIN_SEVERITY),
		actionTouched: false,
		botId: connectedBotId,
		instructions: '',
		notifyIds: currentUserId === null ? [] : [currentUserId],
		labelIds: [],
		tasksPerHour: String(TASKS_PER_HOUR.default),
		activeTasks: String(ACTIVE_TASKS.default),
		attempts: String(ATTEMPTS.default),
		dedupWindowSecs: DEDUP_WINDOW.default,
		rearmAfterSecs: REARM_AFTER.default,
		priorityMap: { ...DEFAULT_PRIORITIES }
	};
}

/** The editor of a stored rule: every stored value, and an action that no longer follows the severity. */
export function draftFromRule(rule: Rule): RuleDraft {
	return {
		name: rule.name,
		enabled: rule.enabled,
		sourceId: rule.source_id,
		kindsText: rule.kind_patterns.join('\n'),
		minSeverity: rule.min_severity,
		action: rule.action,
		actionTouched: true,
		botId: rule.bot_user_id,
		instructions: rule.instructions,
		notifyIds: [...rule.notify_user_ids],
		labelIds: [...rule.label_ids],
		tasksPerHour: String(rule.max_tasks_per_hour),
		activeTasks: String(rule.max_active_tasks),
		attempts: String(rule.task_max_attempts),
		dedupWindowSecs: rule.dedup_window_secs,
		rearmAfterSecs: rule.rearm_after_secs,
		priorityMap: { ...rule.priority_map }
	};
}

/** The default action follows the severity until the person touches the action, never after. */
export function withSeverity(draft: RuleDraft, severity: Severity): RuleDraft {
	return {
		...draft,
		minSeverity: severity,
		action: draft.actionTouched ? draft.action : defaultActionFor(severity)
	};
}

export function withAction(draft: RuleDraft, action: RuleAction): RuleDraft {
	return { ...draft, action, actionTouched: true };
}

export function draftChanged(current: RuleDraft, original: RuleDraft): boolean {
	return JSON.stringify(current) !== JSON.stringify(original);
}

/** The safety limits equal the defaults (the disclosure prints "Defaults" or "Changed"). */
export function limitsAreDefault(draft: RuleDraft): boolean {
	return (
		draft.tasksPerHour === String(TASKS_PER_HOUR.default) &&
		draft.activeTasks === String(ACTIVE_TASKS.default) &&
		draft.attempts === String(ATTEMPTS.default) &&
		draft.dedupWindowSecs === DEDUP_WINDOW.default &&
		draft.rearmAfterSecs === REARM_AFTER.default &&
		SEVERITIES.every((s) => draft.priorityMap[s] === DEFAULT_PRIORITIES[s])
	);
}

// --- kind patterns --------------------------------------------------------------------------

/** An exact kind or a prefix (`waf.*`): up to four dot-separated segments, then optionally `.*`. */
const KIND_PATTERN = /^[a-z0-9_]{1,32}(\.[a-z0-9_]{1,32}){0,3}(\.\*)?$/;
const KIND_MAX_LENGTH = 64;

export function isKindPattern(pattern: string): boolean {
	if (!KIND_PATTERN.test(pattern)) return false;
	const base = pattern.endsWith('.*') ? pattern.slice(0, -2) : pattern;
	return base.length <= KIND_MAX_LENGTH;
}

/** The first exact kind (not a `prefix.*`) of a kinds text, for the rule test sample event. */
export function firstExactKind(text: string): string | null {
	return parseKinds(text).patterns.find((p) => !p.endsWith('.*')) ?? null;
}

export interface ParsedKinds {
	/** Valid patterns in typed order; duplicates are dropped silently. */
	patterns: string[];
	/** Lines that are not a kind or a prefix. */
	invalid: string[];
}

export function parseKinds(text: string): ParsedKinds {
	const patterns: string[] = [];
	const invalid: string[] = [];
	const seen = new Set<string>();
	for (const raw of text.split(/\r?\n/)) {
		const line = raw.trim();
		if (line === '' || seen.has(line)) continue;
		seen.add(line);
		if (isKindPattern(line)) patterns.push(line);
		else invalid.push(line);
	}
	return { patterns, invalid };
}

// --- validation -----------------------------------------------------------------------------

export interface FieldError {
	key: string;
	values?: Record<string, string | number>;
}

export interface RuleErrors {
	name: FieldError | null;
	kinds: FieldError | null;
	bot: FieldError | null;
	instructions: FieldError | null;
	notify: FieldError | null;
	labels: FieldError | null;
	tasksPerHour: FieldError | null;
	activeTasks: FieldError | null;
	attempts: FieldError | null;
	dedupWindow: FieldError | null;
	rearm: FieldError | null;
}

/** A whole, non-negative number typed as digits only, else `null`. */
export function wholeNumber(text: string): number | null {
	const trimmed = text.trim();
	return /^\d{1,9}$/.test(trimmed) ? Number(trimmed) : null;
}

function rangeError(text: string, range: { min: number; max: number }): FieldError | null {
	const value = wholeNumber(text);
	return value !== null && value >= range.min && value <= range.max
		? null
		: { key: 'inbound.error.range', values: { min: range.min, max: range.max } };
}

function durationError(
	seconds: number | null,
	range: { min: number; max: number },
	key: string
): FieldError | null {
	return seconds !== null && seconds >= range.min && seconds <= range.max ? null : { key };
}

const length = (text: string): number => [...text].length;

export function validateDraft(draft: RuleDraft): RuleErrors {
	const name = draft.name.trim();
	const kinds = parseKinds(draft.kindsText);
	return {
		name:
			name === ''
				? { key: 'inbound.error.nameRequired' }
				: length(name) > NAME_MAX
					? { key: 'inbound.error.nameLength' }
					: null,
		kinds:
			kinds.invalid.length > 0
				? { key: 'inbound.error.kindInvalid', values: { pattern: kinds.invalid[0] ?? '' } }
				: kinds.patterns.length > KINDS_MAX
					? { key: 'inbound.error.kindCount' }
					: null,
		bot:
			draft.action === 'work_item_and_task' && draft.botId === null
				? { key: 'inbound.error.botChoose' }
				: null,
		instructions:
			length(draft.instructions) > INSTRUCTIONS_MAX
				? { key: 'inbound.error.instructionsLength' }
				: null,
		notify:
			draft.notifyIds.length > RECIPIENTS_MAX ? { key: 'inbound.error.recipientsCount' } : null,
		labels: draft.labelIds.length > LABELS_MAX ? { key: 'inbound.error.labelsCount' } : null,
		tasksPerHour: rangeError(draft.tasksPerHour, TASKS_PER_HOUR),
		activeTasks: rangeError(draft.activeTasks, ACTIVE_TASKS),
		attempts: rangeError(draft.attempts, ATTEMPTS),
		dedupWindow: durationError(
			draft.dedupWindowSecs,
			DEDUP_WINDOW,
			'inbound.rules.error.dedupRange'
		),
		rearm: durationError(draft.rearmAfterSecs, REARM_AFTER, 'inbound.rules.error.rearmRange')
	};
}

export function draftValid(errors: RuleErrors): boolean {
	return Object.values(errors).every((error) => error === null);
}

/** Whether the limits disclosure holds an error (it opens on a failed save). */
export function limitsInvalid(errors: RuleErrors): boolean {
	return (
		errors.tasksPerHour !== null ||
		errors.activeTasks !== null ||
		errors.attempts !== null ||
		errors.dedupWindow !== null ||
		errors.rearm !== null
	);
}

// --- request bodies -------------------------------------------------------------------------

/** Assembles a body from already-narrowed parts; the one place that lists every field. */
function bodyOf(parts: {
	name: string;
	enabled: boolean;
	sourceId: string | null;
	kindPatterns: string[];
	minSeverity: Severity;
	action: RuleAction;
	botId: string | null;
	instructions: string;
	priorityMap: Record<Severity, Priority>;
	labelIds: string[];
	notifyIds: string[];
	tasksPerHour: number;
	activeTasks: number;
	dedupWindowSecs: number;
	rearmAfterSecs: number;
	attempts: number;
}): RuleBody {
	return {
		name: parts.name,
		enabled: parts.enabled,
		source_id: parts.sourceId,
		kind_patterns: [...parts.kindPatterns],
		min_severity: parts.minSeverity,
		action: parts.action,
		// The server refuses a bot with "open a work item only": send it only with "hand to a bot".
		...(parts.action === 'work_item_and_task' && parts.botId !== null
			? { bot_user_id: parts.botId }
			: {}),
		instructions: parts.instructions,
		priority_map: { ...parts.priorityMap },
		label_ids: [...parts.labelIds],
		notify_user_ids: [...parts.notifyIds],
		max_tasks_per_hour: parts.tasksPerHour,
		max_active_tasks: parts.activeTasks,
		dedup_window_secs: parts.dedupWindowSecs,
		rearm_after_secs: parts.rearmAfterSecs,
		task_max_attempts: parts.attempts
	};
}

/** The body of a draft, or `null` while any client rule fails. Every field is stated. */
export function buildBody(draft: RuleDraft): RuleBody | null {
	const errors = validateDraft(draft);
	if (!draftValid(errors)) return null;
	const tasksPerHour = wholeNumber(draft.tasksPerHour);
	const activeTasks = wholeNumber(draft.activeTasks);
	const attempts = wholeNumber(draft.attempts);
	if (
		tasksPerHour === null ||
		activeTasks === null ||
		attempts === null ||
		draft.dedupWindowSecs === null ||
		draft.rearmAfterSecs === null
	)
		return null;
	return bodyOf({
		name: draft.name.trim(),
		enabled: draft.enabled,
		sourceId: draft.sourceId,
		kindPatterns: parseKinds(draft.kindsText).patterns,
		minSeverity: draft.minSeverity,
		action: draft.action,
		botId: draft.botId,
		instructions: draft.instructions,
		priorityMap: draft.priorityMap,
		labelIds: draft.labelIds,
		notifyIds: draft.notifyIds,
		tasksPerHour,
		activeTasks,
		dedupWindowSecs: draft.dedupWindowSecs,
		rearmAfterSecs: draft.rearmAfterSecs,
		attempts
	});
}

/** The save of an edited rule: the whole body plus the version it was read at. */
export function buildReplaceBody(draft: RuleDraft, rule: Rule): ReplaceRuleBody | null {
	const body = buildBody(draft);
	return body === null ? null : { ...body, expected_version: rule.version };
}

/**
 * The whole stored rule as a body (the enable switch): nothing is dropped, so the server's
 * defaults never replace a value nobody edited.
 */
export function ruleToBody(rule: Rule, patch: { enabled?: boolean } = {}): RuleBody {
	return bodyOf({
		name: rule.name,
		enabled: patch.enabled ?? rule.enabled,
		sourceId: rule.source_id,
		kindPatterns: rule.kind_patterns,
		minSeverity: rule.min_severity,
		action: rule.action,
		botId: rule.bot_user_id,
		instructions: rule.instructions,
		priorityMap: rule.priority_map,
		labelIds: rule.label_ids,
		notifyIds: rule.notify_user_ids,
		tasksPerHour: rule.max_tasks_per_hour,
		activeTasks: rule.max_active_tasks,
		dedupWindowSecs: rule.dedup_window_secs,
		rearmAfterSecs: rule.rearm_after_secs,
		attempts: rule.task_max_attempts
	});
}

/** The enable switch: the whole rule with `enabled` set, and the version it was read at. */
export function toggleBody(rule: Rule, enabled: boolean): ReplaceRuleBody {
	return { ...ruleToBody(rule, { enabled }), expected_version: rule.version };
}

// --- reorder --------------------------------------------------------------------------------

/** The full new order after moving one rule by one place, or `null` at either end or for an unknown id. */
export function movedOrder(ids: readonly string[], id: string, delta: -1 | 1): string[] | null {
	const from = ids.indexOf(id);
	const to = from + delta;
	if (from < 0 || to < 0 || to >= ids.length) return null;
	const next = [...ids];
	[next[from], next[to]] = [next[to] as string, next[from] as string];
	return next;
}

// --- server messages ------------------------------------------------------------------------

export type ServerField =
	| 'name'
	| 'source'
	| 'kinds'
	| 'bot'
	| 'instructions'
	| 'notify'
	| 'labels'
	| 'tasksPerHour'
	| 'activeTasks'
	| 'attempts'
	| 'dedupWindow'
	| 'rearm'
	| 'other';

const SERVER_FIELDS: Record<string, ServerField> = {
	name: 'name',
	source_id: 'source',
	kind_patterns: 'kinds',
	bot_user_id: 'bot',
	instructions: 'instructions',
	notify_user_ids: 'notify',
	label_ids: 'labels',
	max_tasks_per_hour: 'tasksPerHour',
	max_active_tasks: 'activeTasks',
	task_max_attempts: 'attempts',
	dedup_window_secs: 'dedupWindow',
	rearm_after_secs: 'rearm'
};

/** The backend names the field first ("name must be ..."); anything else is a form-level message. */
export function routeServerMessage(message: string): ServerField {
	const first = /^([a-z_]+)\b/.exec(message.trim())?.[1];
	return (first !== undefined ? SERVER_FIELDS[first] : undefined) ?? 'other';
}

export function isLimitsField(field: ServerField): boolean {
	return (
		field === 'tasksPerHour' ||
		field === 'activeTasks' ||
		field === 'attempts' ||
		field === 'dedupWindow' ||
		field === 'rearm'
	);
}

// --- list rows ------------------------------------------------------------------------------

/** The first two patterns and the number left over, for the second line of a row. */
export function kindsPreview(patterns: readonly string[]): { shown: string[]; more: number } {
	return { shown: patterns.slice(0, 2), more: Math.max(0, patterns.length - 2) };
}
