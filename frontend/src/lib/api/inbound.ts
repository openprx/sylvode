import { apiClient, type ApiResult } from './client';
import {
	parseDeleted,
	parseEvents,
	parseIssueAlert,
	parseRuleWrite,
	parseRules,
	parseSecretsAnswer,
	parseSource,
	parseSourceList,
	parseTestResult,
	type InboundEvent,
	type IssueAlert,
	type Priority,
	type Rule,
	type RuleAction,
	type RuleWrite,
	type SecretsAnswer,
	type Severity,
	type Source,
	type SourceList,
	type TestResult
} from '#lib/project/inbound/wire.js';

/**
 * What a call to the inbound source and alert rule API came to. The answer is read at the
 * boundary (`#lib/project/inbound/wire`): a success carries only narrowed values, a rule write
 * against a stale version is its own typed result, and everything else carries the server's
 * message and, for a typed rejection, its stable code.
 *
 * No answer of this API is cached, kept or logged here: the create and rotate answers hold the
 * token and the signing secret, and the caller owns them from the moment they are returned.
 */
export type CallResult<T> =
	| { ok: true; value: T }
	| { ok: false; kind: 'conflict'; code: number; message: string }
	| {
			ok: false;
			kind: 'error';
			code: number;
			message: string;
			/** Set when the client produced the failure itself: `describeResult` makes words of it. */
			messageKey?: string;
			errorCode: string | null;
		};

/** The rejection of a rule write whose `expected_version` is stale (409 `version_conflict`). */
export const VERSION_CONFLICT = 'version_conflict';

function failure(result: ApiResult<unknown>): CallResult<never> {
	if (result.code === 409 && result.error_code === VERSION_CONFLICT)
		return { ok: false, kind: 'conflict', code: result.code, message: result.message };
	return {
		ok: false,
		kind: 'error',
		code: result.code,
		message: result.message,
		...(result.messageKey ? { messageKey: result.messageKey } : {}),
		errorCode: result.error_code ?? null
	};
}

function narrowed<T>(result: ApiResult<unknown>, read: (data: unknown) => T | null): CallResult<T> {
	if (result.code !== 0) return failure(result);
	const value = read(result.data);
	return value === null
		? {
				ok: false,
				kind: 'error',
				code: 500,
				message: '',
				messageKey: 'transport.invalidResponse',
				errorCode: null
			}
		: { ok: true, value };
}

// --- request bodies -------------------------------------------------------------------------

export interface CreateSourceInput {
	name: string;
	require_signature?: boolean;
	rate_per_minute?: number;
	daily_cap?: number;
	ip_allowlist?: string[];
}

/** Only the changed fields; an empty `ip_allowlist` clears the list. */
export interface UpdateSourceInput {
	name?: string;
	enabled?: boolean;
	require_signature?: boolean;
	rate_per_minute?: number;
	daily_cap?: number;
	ip_allowlist?: string[];
	take_over?: boolean;
}

export type PriorityBody = Record<Severity, Priority>;

/**
 * A rule as the editor sends it. A create states every field; a replace states the whole rule
 * plus `expected_version`. `bot_user_id` is present only with `work_item_and_task`.
 */
export interface RuleBody {
	name: string;
	enabled: boolean;
	source_id: string | null;
	kind_patterns: string[];
	min_severity: Severity;
	action: RuleAction;
	bot_user_id?: string;
	instructions: string;
	priority_map: PriorityBody;
	label_ids: string[];
	notify_user_ids: string[];
	max_tasks_per_hour: number;
	max_active_tasks: number;
	dedup_window_secs: number;
	rearm_after_secs: number;
	task_max_attempts: number;
}

export type ReplaceRuleBody = RuleBody & { expected_version: number };

/**
 * What a rule test is run against. Exactly one of a stored event of the source and a sample
 * envelope; `draft_rule` (gap G5) puts the unsaved rule in its place in the order, replacing
 * `replaces` when given and otherwise last.
 */
export type TestInput = { source_id: string } & (
	| { inbound_event_id: string; envelope?: never }
	| { envelope: Record<string, unknown>; inbound_event_id?: never }
) & { draft_rule?: RuleBody; replaces?: string };

const projectPath = (projectId: string): string =>
	`/api/v1/projects/${encodeURIComponent(projectId)}`;
const sourcePath = (id: string): string => `/api/v1/inbound-sources/${encodeURIComponent(id)}`;
const rulePath = (id: string): string => `/api/v1/alert-rules/${encodeURIComponent(id)}`;

export const EVENTS_PAGE = 50;

export const inboundApi = {
	// Sources
	async listSources(projectId: string): Promise<CallResult<SourceList>> {
		return narrowed(
			await apiClient.get<unknown>(`${projectPath(projectId)}/inbound-sources`),
			parseSourceList
		);
	},

	/** The answer holds the token and signing secret: hand it to the one-time component only. */
	async createSource(
		projectId: string,
		input: CreateSourceInput
	): Promise<CallResult<SecretsAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${projectPath(projectId)}/inbound-sources`, input),
			parseSecretsAnswer
		);
	},

	async updateSource(id: string, input: UpdateSourceInput): Promise<CallResult<Source>> {
		return narrowed(await apiClient.patch<unknown>(sourcePath(id), input), parseSource);
	},

	/** The answer holds the new token and signing secret: hand it to the one-time component only. */
	async rotateSource(id: string): Promise<CallResult<SecretsAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${sourcePath(id)}/rotate`, {}),
			parseSecretsAnswer
		);
	},

	async deleteSource(id: string): Promise<CallResult<{ deleted: true }>> {
		return narrowed(await apiClient.delete<unknown>(sourcePath(id)), parseDeleted);
	},

	/** Newest first; `before` is the `received_at` of the last row of the page before. */
	async listEvents(
		sourceId: string,
		options: { before?: string; limit?: number } = {}
	): Promise<CallResult<InboundEvent[]>> {
		const query = new URLSearchParams({ limit: String(options.limit ?? EVENTS_PAGE) });
		if (options.before) query.set('before', options.before);
		return narrowed(
			await apiClient.get<unknown>(`${sourcePath(sourceId)}/events?${query.toString()}`),
			parseEvents
		);
	},

	// Rules
	async listRules(projectId: string): Promise<CallResult<Rule[]>> {
		return narrowed(
			await apiClient.get<unknown>(`${projectPath(projectId)}/alert-rules`),
			parseRules
		);
	},

	async createRule(projectId: string, body: RuleBody): Promise<CallResult<RuleWrite>> {
		return narrowed(
			await apiClient.post<unknown>(`${projectPath(projectId)}/alert-rules`, body),
			parseRuleWrite
		);
	},

	/** A full replace: a stale `expected_version` is the typed `conflict` result. */
	async replaceRule(id: string, body: ReplaceRuleBody): Promise<CallResult<RuleWrite>> {
		return narrowed(await apiClient.put<unknown>(rulePath(id), body), parseRuleWrite);
	},

	/** The full new order; the answer is the rules in that order. */
	async reorderRules(projectId: string, ruleIds: string[]): Promise<CallResult<Rule[]>> {
		return narrowed(
			await apiClient.post<unknown>(`${projectPath(projectId)}/alert-rules/reorder`, {
				rule_ids: ruleIds
			}),
			parseRules
		);
	},

	async deleteRule(id: string): Promise<CallResult<{ deleted: true }>> {
		return narrowed(await apiClient.delete<unknown>(rulePath(id)), parseDeleted);
	},

	/** Alert facts of one work item. A 404 `code` means the item is not an alert. */
	async getIssueAlert(issueId: string): Promise<CallResult<IssueAlert>> {
		return narrowed(
			await apiClient.get<unknown>(`/api/v1/issues/${encodeURIComponent(issueId)}/alert`),
			parseIssueAlert
		);
	},

	// Rule test (nothing is written, no limit is counted)
	async testRules(projectId: string, input: TestInput): Promise<CallResult<TestResult>> {
		return narrowed(
			await apiClient.post<unknown>(`${projectPath(projectId)}/alert-rules/test`, input),
			parseTestResult
		);
	}
};
