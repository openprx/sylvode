import { apiClient, type ApiResult } from './client';
import {
	isRecord,
	parseConnection,
	parseList,
	parseRead,
	parseRemove,
	parseWrite,
	type Channel,
	type Cli,
	type Connection,
	type Level,
	type ListAnswer,
	type ReadAnswer,
	type RemoveAnswer,
	type WriteAnswer
} from '#lib/project/connection/wire.js';

/**
 * What a call to the bot connection API came to. The answer is read at the boundary
 * (`#lib/project/connection/wire`): a success carries only narrowed values, a stale version
 * carries the connection as it now stands, and everything else carries the server's message.
 *
 * No answer of this API is cached, kept or logged here: the connect and rotate answers hold the
 * token and the signing secret, and the caller owns them from the moment they are returned.
 */
export type CallResult<T> =
	| { ok: true; value: T }
	| { ok: false; code: number; message: string; messageKey?: string; current: Connection | null };

export interface GovernanceInput {
	max_domain_level?: Level;
	can_veto_human_consensus?: boolean;
	reason_min_length?: number;
	domain_overrides?: Record<string, Level>;
}

export interface ConnectInput {
	bot: { name: string } | { id: string };
	runner_url: string;
	cli: Cli;
	working_directory: string;
	channel?: Channel;
	on_complete_state?: string;
	on_fail_label_id?: string | null;
	governance?: GovernanceInput;
}

export interface UpdateInput {
	expected_version: number;
	runner_url?: string;
	cli?: Cli;
	working_directory?: string;
	channel?: Channel;
	on_complete_state?: string;
	on_fail_label_id?: string | null;
	governance?: GovernanceInput;
}

function failure(result: ApiResult<unknown>): CallResult<never> {
	const current =
		result.code === 409 && isRecord(result.data) ? parseConnection(result.data.current) : null;
	return {
		ok: false,
		code: result.code,
		message: result.message,
		...(result.messageKey ? { messageKey: result.messageKey } : {}),
		current
	};
}

function narrowed<T>(result: ApiResult<unknown>, read: (data: unknown) => T | null): CallResult<T> {
	if (result.code !== 0) return failure(result);
	const value = read(result.data);
	return value === null
		? { ok: false, code: 500, message: '', messageKey: 'transport.invalidResponse', current: null }
		: { ok: true, value };
}

const base = (projectId: string): string =>
	`/api/v1/projects/${encodeURIComponent(projectId)}/bot-connections`;
const path = (projectId: string, botId: string): string =>
	`${base(projectId)}/${encodeURIComponent(botId)}`;

export const botConnectionApi = {
	/** Every connection of the project, ordered by bot name. */
	async list(projectId: string): Promise<CallResult<ListAnswer>> {
		return narrowed(await apiClient.get<unknown>(base(projectId)), parseList);
	},

	async get(projectId: string, botId: string): Promise<CallResult<ReadAnswer>> {
		return narrowed(await apiClient.get<unknown>(path(projectId, botId)), parseRead);
	},

	async connect(projectId: string, input: ConnectInput): Promise<CallResult<WriteAnswer>> {
		return narrowed(await apiClient.post<unknown>(base(projectId), input), parseWrite);
	},

	async update(
		projectId: string,
		botId: string,
		input: UpdateInput
	): Promise<CallResult<WriteAnswer>> {
		return narrowed(await apiClient.patch<unknown>(path(projectId, botId), input), parseWrite);
	},

	async disable(
		projectId: string,
		botId: string,
		expectedVersion: number
	): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId, botId)}/disable`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async enable(
		projectId: string,
		botId: string,
		expectedVersion: number
	): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId, botId)}/enable`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async rotate(
		projectId: string,
		botId: string,
		expectedVersion: number
	): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId, botId)}/rotate`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async remove(
		projectId: string,
		botId: string,
		expectedVersion: number
	): Promise<CallResult<RemoveAnswer>> {
		return narrowed(
			await apiClient.delete<unknown>(
				`${path(projectId, botId)}?expected_version=${expectedVersion}`
			),
			parseRemove
		);
	}
};
