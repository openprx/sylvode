/** Shapes of the navigation API (`sv.nav.v1`, `sv.nav.prefs.v1`, `sv.nav.resolved.v1`). */

export type NavScope = 'workspace' | 'project';
export type LocaleLabels = Record<string, string>;

export type NavTarget =
	| { kind: 'builtin'; page: string }
	| { kind: 'project'; project_id: string }
	| { kind: 'form'; form_id: string }
	| { kind: 'flow_page'; object_id: string }
	| { kind: 'layout_page'; layout_id: string }
	| { kind: 'plugin_app'; package_key: string; route: string }
	| { kind: 'external'; url: string };

export interface NavEntryDoc {
	id: string;
	target: NavTarget;
	label: LocaleLabels | null;
	icon: string | null;
	hidden: boolean;
	pinned: boolean;
	locked: boolean;
}

export interface NavSectionDoc {
	id: string;
	label: LocaleLabels | null;
	collapsed: boolean;
	entries: NavEntryDoc[];
}

export interface NavDoc {
	schema: 'sv.nav.v1';
	scope: NavScope;
	sections: NavSectionDoc[];
}

export interface NavConfig {
	doc: NavDoc;
	version: number;
	is_default: boolean;
	updated_at: string | null;
	broken: string[];
}

export interface NavPrefs {
	schema: 'sv.nav.prefs.v1';
	hidden: string[];
	order: Record<string, string[]>;
	pinned: string[];
	unpinned: string[];
	collapsed: string[];
	/** Sections collapsed by default that this person keeps open. */
	expanded: string[];
}

export interface ResolvedEntry {
	id: string;
	/** The section the entry is stored in, also when it is listed in `_pinned`. */
	section_id: string;
	kind: NavTarget['kind'];
	target: NavTarget;
	label: LocaleLabels | null;
	name: string | null;
	icon: string | null;
	href: string | null;
	pinned: boolean;
	locked: boolean;
	user_hidden: boolean;
	/** Index in its stored section after the person's order is applied (also when pinned). */
	position: number;
}

export interface ResolvedSection {
	id: string;
	label: LocaleLabels | null;
	collapsed: boolean;
	entries: ResolvedEntry[];
}

export interface ResolvedNav {
	schema: 'sv.nav.resolved.v1';
	scope: NavScope;
	scope_id: string;
	config_version: number;
	customised: boolean;
	sections: ResolvedSection[];
}

export const PINNED_SECTION = '_pinned';
