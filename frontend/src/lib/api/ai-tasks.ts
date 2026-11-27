import { apiClient, type ApiResult } from './client';
import { transportFailure } from './transport';

/**
 * One event of an agent task, as `GET /api/v1/issues/{issue_id}/ai-tasks` returns it
 * (`apps/api/src/routes/ai_callback.rs::list_issue_ai_tasks`). The payload shape depends on the
 * event type, so it stays a record of unknown values and is narrowed where it is read.
 */
export interface AiTaskEvent {
	event_type: string;
	payload: Record<string, unknown>;
	created_at: string;
}

/**
 * One agent task of a work item (`AiTaskResponse` plus `events`: the task's newest events, oldest
 * first, and `events_omitted`: whether older ones were left out). The free-text
 * fields (`plan`, `error_message`, event payloads) are written by an agent and are untrusted.
 */
export interface AiTask {
	id: string;
	project_id: string;
	ai_participant_id: string;
	task_type: string;
	status: string;
	attempts: number;
	max_attempts: number;
	error_message: string | null;
	next_retry_at: string | null;
	started_at: string | null;
	completed_at: string | null;
	created_at: string;
	updated_at: string;
	acknowledged_at: string | null;
	plan: string | null;
	last_progress_at: string | null;
	lease_expires_at: string | null;
	/** `queued`, `pushed` or `blocked` while a runner reports one; anything else is kept as sent. */
	runner_phase: string | null;
	events: AiTaskEvent[];
	/** True when the task has older events than the ones in `events`. */
	events_omitted: boolean;
}

type Raw = Record<string, unknown>;

function isRecord(value: unknown): value is Raw {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function str(raw: Raw, key: string): string | null {
	const value = raw[key];
	return typeof value === 'string' ? value : null;
}

function int(raw: Raw, key: string): number | null {
	const value = raw[key];
	return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

/** Optional text: absent or null is `null`; any other type is a malformed response. */
function optionalStr(raw: Raw, key: string): string | null | undefined {
	const value = raw[key];
	if (value === undefined || value === null) return null;
	return typeof value === 'string' ? value : undefined;
}

export function narrowAiTaskEvent(value: unknown): AiTaskEvent | null {
	if (!isRecord(value)) return null;
	const eventType = str(value, 'event_type');
	const createdAt = str(value, 'created_at');
	if (eventType === null || createdAt === null) return null;
	const payload = value.payload;
	return {
		event_type: eventType,
		payload: isRecord(payload) ? payload : {},
		created_at: createdAt
	};
}

export function narrowAiTask(value: unknown): AiTask | null {
	if (!isRecord(value)) return null;
	const id = str(value, 'id');
	const projectId = str(value, 'project_id');
	const participantId = str(value, 'ai_participant_id');
	const taskType = str(value, 'task_type');
	const status = str(value, 'status');
	const attempts = int(value, 'attempts');
	const maxAttempts = int(value, 'max_attempts');
	const createdAt = str(value, 'created_at');
	const updatedAt = str(value, 'updated_at');
	if (
		id === null ||
		projectId === null ||
		participantId === null ||
		taskType === null ||
		status === null ||
		attempts === null ||
		maxAttempts === null ||
		createdAt === null ||
		updatedAt === null
	) {
		return null;
	}
	const optional = {
		error_message: optionalStr(value, 'error_message'),
		next_retry_at: optionalStr(value, 'next_retry_at'),
		started_at: optionalStr(value, 'started_at'),
		completed_at: optionalStr(value, 'completed_at'),
		acknowledged_at: optionalStr(value, 'acknowledged_at'),
		plan: optionalStr(value, 'plan'),
		last_progress_at: optionalStr(value, 'last_progress_at'),
		lease_expires_at: optionalStr(value, 'lease_expires_at'),
		runner_phase: optionalStr(value, 'runner_phase')
	};
	if (Object.values(optional).some((entry) => entry === undefined)) return null;
	const eventsOmitted = value.events_omitted;
	if (typeof eventsOmitted !== 'boolean') return null;
	const rawEvents = value.events === undefined ? [] : value.events;
	if (!Array.isArray(rawEvents)) return null;
	const events: AiTaskEvent[] = [];
	for (const rawEvent of rawEvents) {
		const event = narrowAiTaskEvent(rawEvent);
		if (event === null) return null;
		events.push(event);
	}
	return {
		id,
		project_id: projectId,
		ai_participant_id: participantId,
		task_type: taskType,
		status,
		attempts,
		max_attempts: maxAttempts,
		created_at: createdAt,
		updated_at: updatedAt,
		error_message: optional.error_message ?? null,
		next_retry_at: optional.next_retry_at ?? null,
		started_at: optional.started_at ?? null,
		completed_at: optional.completed_at ?? null,
		acknowledged_at: optional.acknowledged_at ?? null,
		plan: optional.plan ?? null,
		last_progress_at: optional.last_progress_at ?? null,
		lease_expires_at: optional.lease_expires_at ?? null,
		runner_phase: optional.runner_phase ?? null,
		events,
		events_omitted: eventsOmitted
	};
}

/** One page of tasks, newest first, in the `PaginatedData` shape. */
export interface AiTaskPage {
	items: AiTask[];
	total: number;
	page: number;
	per_page: number;
	total_pages: number;
}

/** A task list entry that is malformed makes the whole answer invalid. */
export function narrowAiTaskList(value: unknown): AiTask[] | null {
	if (!Array.isArray(value)) return null;
	const tasks: AiTask[] = [];
	for (const entry of value) {
		const task = narrowAiTask(entry);
		if (task === null) return null;
		tasks.push(task);
	}
	return tasks;
}

export function narrowAiTaskPage(value: unknown): AiTaskPage | null {
	if (!isRecord(value)) return null;
	const items = narrowAiTaskList(value.items);
	const total = int(value, 'total');
	const page = int(value, 'page');
	const perPage = int(value, 'per_page');
	const totalPages = int(value, 'total_pages');
	if (
		items === null ||
		total === null ||
		page === null ||
		perPage === null ||
		totalPages === null
	) {
		return null;
	}
	return { items, total, page, per_page: perPage, total_pages: totalPages };
}

export const aiTasksApi = {
	/** One page of the tasks of a work item, newest first, each with its newest 100 events. */
	async listForIssue(issueId: string, page = 1, perPage = 5): Promise<ApiResult<AiTaskPage>> {
		const result = await apiClient.get<unknown>(
			`/api/v1/issues/${encodeURIComponent(issueId)}/ai-tasks?page=${page}&per_page=${perPage}`
		);
		if (result.code !== 0) {
			return { ...result, data: null };
		}
		const tasks = narrowAiTaskPage(result.data);
		if (tasks === null) {
			return transportFailure(500, 'transport.invalidResponse');
		}
		return { code: 0, message: result.message, data: tasks };
	}
};
