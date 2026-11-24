/** Independent HTTP protocol; this adapter has no Page/bootstrap/history fallback. Not routed yet. */
import { apiClient, type ApiResult } from '#lib/api/client.js';
import { transportFailure } from '#lib/api/transport.js';
import { bodyBinaryResult } from '../body/bytes';
import {
	parseSharedRead,
	type SharedRead,
	type SharedPatch,
	type CreateBlockShare,
	type ManagedBlockShare,
	parseSharedDiscovery,
	parseSharedPlacementManagement,
	type SharedPlacementManagement,
	type SharedTargetContext,
	type SharedDiscovery
} from './contracts';
const root = '/api/v2/flow/block-shares';
function checked(result: ApiResult<unknown>, shareId: string): ApiResult<SharedRead> {
	if (result.code !== 0 || !result.data) return { ...result, data: null };
	try {
		return { ...result, data: parseSharedRead(result.data, shareId) };
	} catch {
		return transportFailure(502, 'transport.sharedBlockInvalid');
	}
}
export const sharedBlockApi = {
	async read(
		shareId: string,
		signal?: AbortSignal,
		context?: SharedTargetContext
	): Promise<ApiResult<SharedRead>> {
		return checked(
			await apiClient.request(
				'GET',
				`${root}/${encodeURIComponent(shareId)}${targetQuery(context)}`,
				undefined,
				true,
				{},
				signal
			),
			shareId
		);
	},
	async patch(
		shareId: string,
		patch: SharedPatch,
		key: string,
		signal?: AbortSignal
	): Promise<ApiResult<SharedRead>> {
		return checked(
			await apiClient.request(
				'PATCH',
				`${root}/${encodeURIComponent(shareId)}`,
				patch,
				true,
				{ 'Idempotency-Key': key },
				signal
			),
			shareId
		);
	},
	asset(shareId: string, alias: string, signal?: AbortSignal) {
		return apiClient
			.getBinary(
				`${root}/${encodeURIComponent(shareId)}/assets/${encodeURIComponent(alias)}`,
				true,
				signal
			)
			.then(bodyBinaryResult);
	},
	create(sourceId: string, value: CreateBlockShare, key: string) {
		return apiClient.request<ManagedBlockShare>(
			'POST',
			`/api/v2/flow/objects/${encodeURIComponent(sourceId)}/block-shares`,
			value,
			true,
			{ 'Idempotency-Key': key }
		);
	},
	manage(sourceId: string) {
		return apiClient.get<ManagedBlockShare[]>(
			`/api/v2/flow/objects/${encodeURIComponent(sourceId)}/block-shares`
		);
	},
	changeSourceLifecycle(
		sourceId: string,
		shareId: string,
		action: 'suspend' | 'revoke',
		key: string
	) {
		return apiClient.request<ManagedBlockShare>(
			'POST',
			`/api/v2/flow/objects/${encodeURIComponent(sourceId)}/block-shares/${encodeURIComponent(shareId)}/${action}`,
			{},
			true,
			{ 'Idempotency-Key': key }
		);
	},
	republish(sourceId: string, shareId: string, value: CreateBlockShare, key: string) {
		return apiClient.request<ManagedBlockShare>(
			'POST',
			`/api/v2/flow/objects/${encodeURIComponent(sourceId)}/block-shares/${encodeURIComponent(shareId)}/republish`,
			value,
			true,
			{ 'Idempotency-Key': key }
		);
	},
	async discover(
		context: SharedTargetContext,
		signal?: AbortSignal,
		cursor?: string
	): Promise<ApiResult<SharedDiscovery[]>> {
		const result = await apiClient.request<unknown>(
			'GET',
			`${root}${targetQuery(context)}${cursor ? '&cursor=' + encodeURIComponent(cursor) : ''}`,
			undefined,
			true,
			{},
			signal
		);
		if (result.code !== 0 || !result.data) return { ...result, data: null };
		try {
			return { ...result, data: parseSharedDiscovery(result.data) };
		} catch {
			return transportFailure(502, 'transport.sharedBlockInvalid');
		}
	},
	async placements(
		context: SharedTargetContext,
		signal?: AbortSignal,
		cursor?: string
	): Promise<ApiResult<SharedPlacementManagement[]>> {
		const result = await apiClient.request<unknown>(
			'GET',
			`/api/v2/flow/block-share-placements${targetQuery(context)}${cursor ? '&cursor=' + encodeURIComponent(cursor) : ''}`,
			undefined,
			true,
			{},
			signal
		);
		if (result.code !== 0 || !result.data) return { ...result, data: null };
		try {
			return { ...result, data: parseSharedPlacementManagement(result.data) };
		} catch {
			return transportFailure(502, 'transport.sharedBlockInvalid');
		}
	},

	removePlacement(shareId: string, placementId: string, key: string) {
		return apiClient.request(
			'DELETE',
			`${root}/${encodeURIComponent(shareId)}/placements/${encodeURIComponent(placementId)}`,
			undefined,
			true,
			{ 'Idempotency-Key': key }
		);
	}
};
function targetQuery(context?: SharedTargetContext): string {
	if (!context) return '';
	const query = new URLSearchParams({ workspace_id: context.workspace_id });
	if (context.project_id) query.set('project_id', context.project_id);
	return '?' + query;
}
