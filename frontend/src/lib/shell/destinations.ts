import type { Destination } from './route-map';

export type { Destination };

/** 'personal' or a workspace id. */
export type ScopeKey = string;
export const PERSONAL_SCOPE = 'personal';

/** The three default destinations: fixed, not removable. Governance is an addable shortcut. */
export const DEFAULT_DESTINATIONS = [
	'home',
	'projects',
	'flow'
] as const satisfies readonly Destination[];

export interface DestinationMeta {
	id: Destination;
	/** i18n key of the label. */
	labelKey: string;
	/** Lucide glyph name. */
	glyph: string;
	/** Fixed destinations cannot be removed from the rail. */
	fixed: boolean;
}

export const DESTINATIONS: Record<Destination, DestinationMeta> = {
	home: { id: 'home', labelKey: 'shell.destinations.home', glyph: 'house', fixed: true },
	projects: {
		id: 'projects',
		labelKey: 'shell.destinations.projects',
		glyph: 'folder-kanban',
		fixed: true
	},
	flow: { id: 'flow', labelKey: 'shell.destinations.flow', glyph: 'workflow', fixed: true },
	governance: {
		id: 'governance',
		labelKey: 'shell.destinations.governance',
		glyph: 'landmark',
		fixed: false
	}
};

export const SETTINGS_LABEL_KEY = 'shell.destinations.settings';
export const SETTINGS_ROOT = '/settings';

/** What decides whether Flow is reachable from a scope. */
export interface FlowAvailability {
	/** Personal Flow capability of the signed-in user; false until known. */
	personal: boolean;
	/** Per-workspace Flow flag; undefined while unknown. */
	workspaces: Readonly<Record<string, boolean | undefined>>;
}

export type DestinationRoot =
	| { available: true; href: string }
	| { available: false; reasonKey: string };

export function scopeKeyOf(workspaceId: string | null | undefined): ScopeKey {
	return workspaceId || PERSONAL_SCOPE;
}

/** Where a destination goes with no remembered location in `scope`. */
export function destinationRoot(
	destination: Destination,
	scope: ScopeKey,
	flow: FlowAvailability
): DestinationRoot {
	const personal = scope === PERSONAL_SCOPE;
	const ws = encodeURIComponent(scope);
	switch (destination) {
		case 'home':
			return { available: true, href: '/workspace' };
		case 'projects':
			return { available: true, href: personal ? '/workspace' : `/workspace/${ws}/projects` };
		case 'flow':
			if (personal)
				return flow.personal
					? { available: true, href: '/flow' }
					: { available: false, reasonKey: 'shell.destinations.flowUnavailablePersonal' };
			return flow.workspaces[scope] === true
				? { available: true, href: `/workspace/${ws}/flow` }
				: { available: false, reasonKey: 'shell.destinations.flowUnavailableWorkspace' };
		case 'governance':
			return { available: true, href: '/governance' };
	}
}

/** A rail item's href: the remembered last location when the destination is reachable, else its root. */
export function railHref(
	destination: Destination,
	scope: ScopeKey,
	last: string | undefined,
	flow: FlowAvailability
): DestinationRoot {
	const root = destinationRoot(destination, scope, flow);
	if (!root.available) return root;
	return { available: true, href: last || root.href };
}
