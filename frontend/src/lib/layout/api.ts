import { apiClient, type ApiResult, type PaginatedData } from '#lib/api/client.js';
import type {
	LayoutRecord,
	LayoutSummary,
	LayoutVerdict,
	PlacementPrefs,
	PlacementRecord,
	ScopeKind,
	SlotContextParams,
	SlotDescriptor,
	SlotPlacements
} from './types';

export const layoutApi = {
	slotPlacements(slotId: string, context: SlotContextParams): Promise<ApiResult<SlotPlacements>> {
		const query = new URLSearchParams({ project_id: context.project_id });
		if (context.form_id) query.set('form_id', context.form_id);
		if (context.record_id) query.set('record_id', context.record_id);
		return apiClient.get<SlotPlacements>(
			`/api/v1/slots/${encodeURIComponent(slotId)}/placements?${query.toString()}`
		);
	},

	updatePrefs(
		placementId: string,
		prefs: { hidden?: boolean; collapsed?: boolean; position?: number }
	): Promise<ApiResult<PlacementPrefs>> {
		return apiClient.put<PlacementPrefs>(
			`/api/v1/me/placements/${encodeURIComponent(placementId)}/prefs`,
			prefs
		);
	},

	slots(): Promise<ApiResult<{ slots: SlotDescriptor[] }>> {
		return apiClient.get<{ slots: SlotDescriptor[] }>('/api/v1/slots');
	},

	listLayouts(
		scopeKind: ScopeKind,
		scopeId: string
	): Promise<ApiResult<PaginatedData<LayoutSummary>>> {
		const query = new URLSearchParams({
			scope_kind: scopeKind,
			scope_id: scopeId,
			per_page: '200'
		});
		return apiClient.get<PaginatedData<LayoutSummary>>(`/api/v1/layouts?${query.toString()}`);
	},

	getLayout(id: string): Promise<ApiResult<LayoutRecord>> {
		return apiClient.get<LayoutRecord>(`/api/v1/layouts/${encodeURIComponent(id)}`);
	},

	/** Always answers; the verdict is in `valid` and each error carries a JSON-pointer `path`. */
	validate(body: {
		spec: unknown;
		slot_id?: string;
		scope_kind?: ScopeKind;
		scope_id?: string;
	}): Promise<ApiResult<LayoutVerdict>> {
		return apiClient.post<LayoutVerdict>('/api/v1/layouts/validate', body);
	},

	createLayout(body: {
		scope_kind: ScopeKind;
		scope_id: string;
		spec: unknown;
	}): Promise<ApiResult<LayoutRecord>> {
		return apiClient.post<LayoutRecord>('/api/v1/layouts', body);
	},

	createPlacement(body: {
		slot_id: string;
		scope_kind: ScopeKind;
		scope_id: string;
		layout_id: string;
		condition?: string;
	}): Promise<ApiResult<PlacementRecord>> {
		return apiClient.post<PlacementRecord>('/api/v1/placements', body);
	},

	deletePlacement(id: string): Promise<ApiResult<{ id: string }>> {
		return apiClient.delete<{ id: string }>(`/api/v1/placements/${encodeURIComponent(id)}`);
	},

	/** Plugin action endpoint (lane B). Absent until that lane lands: see `invokeMissing`. */
	invokeAction(
		installationId: string,
		action: string,
		body: { input: unknown; context: SlotContextParams }
	): Promise<ApiResult<unknown>> {
		return apiClient.post<unknown>(
			`/api/v1/installations/${encodeURIComponent(installationId)}/actions/${encodeURIComponent(action)}`,
			body
		);
	}
};

/** The route itself is missing (no typed code), as opposed to a typed refusal from the handler. */
export function invokeMissing(result: { code: number; error_code?: string }): boolean {
	return (result.code === 404 || result.code === 405) && !result.error_code;
}
