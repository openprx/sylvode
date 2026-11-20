import type { ProjectOption } from '#lib/stores/project-options.js';
import type { FlowAvailability } from '../destinations';

/** What the column needs to know about the signed-in user, provided by the shell session. */
export interface ColumnViewer {
	/** Owner or admin of the current scope's workspace (false in the personal scope). */
	workspaceAdmin: boolean;
	/** Instance admin (`isAdminUser`). */
	instanceAdmin: boolean;
	flow: FlowAvailability;
	/** Unread notifications. */
	unread: number;
}

/** The part of a URL the column reads; SvelteKit's `page.url` fits. */
export type RouteUrl = Pick<URL, 'pathname' | 'search'>;

/** The project option store as the column uses it; the app's `projectOptionsStore` fits. */
export interface ProjectOptionsSource {
	subscribe(
		run: (value: {
			items: ProjectOption[];
			loading: boolean;
			loaded: boolean;
			error: string | null;
		}) => void
	): () => void;
	ensureLoaded(force?: boolean): Promise<unknown>;
}
