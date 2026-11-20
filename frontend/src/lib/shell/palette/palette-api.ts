import { flowApi } from '../../api/flow';
import { searchApi, type SearchResult } from '../../api/search';
import type { FlowSearchHit } from '../../api/flow';

/** The two server calls the palette makes; the app passes `defaultPaletteSources`. */
export interface PaletteSources {
	/** Issues and comments (and projects, which the palette ignores). */
	searchAll(query: string, signal: AbortSignal): Promise<SearchResult[] | null>;
	/** Flow pages of one workspace, at most `limit`. */
	searchFlow(
		workspaceId: string,
		query: string,
		limit: number,
		signal: AbortSignal
	): Promise<FlowSearchHit[] | null>;
}

export const defaultPaletteSources: PaletteSources = {
	async searchAll(query) {
		const response = await searchApi.search(query);
		return response.code === 0 && response.data ? (response.data.results ?? []) : null;
	},
	async searchFlow(workspaceId, query, limit, signal) {
		const response = await flowApi.searchObjects(
			workspaceId,
			{ q: query, all_visible: true, limit },
			signal
		);
		return response.code === 0 && response.data ? response.data.items : null;
	}
};
