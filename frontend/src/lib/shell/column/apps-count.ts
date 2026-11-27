import type { AppRef } from '../../nav/rows';
import { layoutSlot, type DiscoveryState } from '#lib/plugins/contributions.js';

/**
 * Number of `project.view` apps a discovery state shows (the Apps row of the project tree).
 * Null while loading or after a failed read; zero when extensions are off or access is denied.
 */
export function appsCountOf(state: DiscoveryState): number | null {
	if (state.status === 'ready') {
		const layout = layoutSlot(state.items, 'project.view');
		return layout.inline.length + layout.more.length;
	}
	if (state.status === 'off' || state.status === 'denied') return 0;
	return null;
}

/** The `project.view` apps a discovery state shows, in display order; null while unknown. */
export function appsOf(state: DiscoveryState): AppRef[] | null {
	if (state.status === 'ready') {
		const layout = layoutSlot(state.items, 'project.view');
		return [...layout.inline, ...layout.more].map((item) => ({
			key: item.key,
			route: item.contribution.route ?? '',
			label: item.contribution.label
		}));
	}
	if (state.status === 'off' || state.status === 'denied') return [];
	return null;
}
