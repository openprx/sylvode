import { apiClient, type ApiResult } from '#lib/api/client.js';
import type { PlacementPrefs, SlotContextParams, SlotPlacements } from './types';

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
