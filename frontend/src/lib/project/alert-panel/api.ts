import { apiClient, type ApiResult } from '../../api/client';

/**
 * The read-only alert facts of a work item, as `GET /api/v1/issues/{id}/alert` returns them
 * (`apps/api/src/inbound/alert_view.rs`). `source`, `kind`, `rule` and every name in them were
 * written by an external system or an administrator and are shown as plain text only.
 *
 * Local to the Alert panel until the shared `lib/api/inbound.ts` exists: whoever merges second
 * folds `getIssueAlert` into that file and deletes this one.
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

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(raw: Raw, key: string): string | null {
	const value = raw[key];
	return typeof value === 'string' ? value : null;
}

/** Absent or null is `null`; any other type is malformed (`undefined`). */
function optionalStr(raw: Raw, key: string): string | null | undefined {
	const value = raw[key];
	if (value === undefined || value === null) return null;
	return typeof value === 'string' ? value : undefined;
}

function idName(value: unknown): { id: string; name: string } | null | undefined {
	if (value === undefined || value === null) return null;
	if (!isRecord(value)) return undefined;
	const id = str(value, 'id');
	const name = str(value, 'name');
	return id === null || name === null ? undefined : { id, name };
}

export function narrowIssueAlert(value: unknown): IssueAlert | null {
	if (!isRecord(value)) return null;
	const issueId = str(value, 'issue_id');
	const alertId = str(value, 'alert_id');
	const firstSeen = str(value, 'first_seen_at');
	const lastSeen = str(value, 'last_seen_at');
	const taskState = str(value, 'task_state');
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
	const kind = optionalStr(value, 'kind');
	const severity = optionalStr(value, 'severity');
	const queuedAt = optionalStr(value, 'queued_at');
	const expiresAt = optionalStr(value, 'queue_expires_at');
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
		const id = str(value.task, 'id');
		const status = str(value.task, 'status');
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

export const alertPanelApi = {
	/** Alert facts of one work item. 404 means the item is not an alert. */
	async getIssueAlert(issueId: string): Promise<ApiResult<IssueAlert>> {
		const result = await apiClient.get<unknown>(
			`/api/v1/issues/${encodeURIComponent(issueId)}/alert`
		);
		if (result.code !== 0) return { ...result, data: null };
		const alert = narrowIssueAlert(result.data);
		if (alert === null) return { code: 500, message: 'Invalid response format', data: null };
		return { code: 0, message: result.message, data: alert };
	}
};
