import { apiClient, type ApiResult } from './client';
import {
	isRecord,
	parseConnection,
	parseRead,
	parseRemove,
	parseWrite,
	type Channel,
	type Cli,
	type Connection,
	type Level,
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
	| { ok: false; code: number; message: string; current: Connection | null };

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
	return { ok: false, code: result.code, message: result.message, current };
}

function narrowed<T>(result: ApiResult<unknown>, read: (data: unknown) => T | null): CallResult<T> {
	if (result.code !== 0) return failure(result);
	const value = read(result.data);
	return value === null
		? { ok: false, code: 500, message: 'The answer was not understood', current: null }
		: { ok: true, value };
}

const path = (projectId: string): string =>
	`/api/v1/projects/${encodeURIComponent(projectId)}/bot-connection`;

export const botConnectionApi = {
	async get(projectId: string): Promise<CallResult<ReadAnswer>> {
		return narrowed(await apiClient.get<unknown>(path(projectId)), parseRead);
	},

	async connect(projectId: string, input: ConnectInput): Promise<CallResult<WriteAnswer>> {
		return narrowed(await apiClient.post<unknown>(path(projectId), input), parseWrite);
	},

	async update(projectId: string, input: UpdateInput): Promise<CallResult<WriteAnswer>> {
		return narrowed(await apiClient.patch<unknown>(path(projectId), input), parseWrite);
	},

	async disable(projectId: string, expectedVersion: number): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId)}/disable`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async enable(projectId: string, expectedVersion: number): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId)}/enable`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async rotate(projectId: string, expectedVersion: number): Promise<CallResult<WriteAnswer>> {
		return narrowed(
			await apiClient.post<unknown>(`${path(projectId)}/rotate`, {
				expected_version: expectedVersion
			}),
			parseWrite
		);
	},

	async remove(projectId: string, expectedVersion: number): Promise<CallResult<RemoveAnswer>> {
		return narrowed(
			await apiClient.delete<unknown>(`${path(projectId)}?expected_version=${expectedVersion}`),
			parseRemove
		);
	}
};
