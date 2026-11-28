/** Wire types of `sv.layout.v1` as the console renderer reads them (C3 §1-§3). */

export type I18nText = Record<string, string>;
/** A text value: literal, i18n map, reference string, or a computed `{cel}` (shown empty here). */
export type TextValue = string | I18nText | { cel: string } | null | undefined;

export interface LayoutNode {
	type: string;
	props?: Record<string, unknown>;
	children?: LayoutNode[];
	when?: string;
	on_click?: string;
}

export interface LayoutParamSpec {
	type?: 'string' | 'int' | 'bool';
	required?: boolean;
}

export interface FilterSpec {
	field: string;
	op: 'eq' | 'neq' | 'contains' | 'gt' | 'gte' | 'lt' | 'lte' | 'not_empty';
	value?: unknown;
}

export type DataSourceSpec =
	| { source: 'record'; form?: string; record: string; fields?: string[] }
	| {
			source: 'query';
			form: string;
			filter?: FilterSpec[];
			sort?: { field: string; dir?: 'asc' | 'desc' }[];
			fields?: string[];
			page_size?: number;
	  }
	| { source: 'form'; form: string; fields?: string[] }
	| { source: 'plugin'; [key: string]: unknown };

export type ActionSpec =
	| {
			kind: 'invoke';
			package: string;
			export: string;
			input?: Record<string, unknown>;
			confirm?: TextValue;
	  }
	| {
			kind: 'submit';
			data: string;
			mode: 'create' | 'update';
			record?: string;
			confirm?: TextValue;
	  }
	| { kind: 'navigate'; to: NavigateTarget };

export type NavigateTarget =
	| { record: { form: string; id: string } }
	| { form: string }
	| { view: { form: string; view: string } }
	| { route: string };

export interface LayoutSpec {
	schema: string;
	key: string;
	title?: TextValue;
	params?: Record<string, LayoutParamSpec>;
	data?: Record<string, DataSourceSpec>;
	actions?: Record<string, ActionSpec>;
	root: LayoutNode;
}

/** One entry of `GET /api/v1/slots/{id}/placements`. */
export interface PlacementEntry {
	placement_id: string;
	layout_id: string;
	layout_key: string;
	spec: LayoutSpec;
	position: number;
	condition: string | null;
	collapsed: boolean;
	installation_id: string | null;
}

export interface SlotPlacements {
	slot: unknown;
	entries: PlacementEntry[];
	installations: Record<string, string>;
}

export interface SlotContextParams {
	project_id: string;
	form_id?: string;
	record_id?: string;
}

export interface PlacementPrefs {
	placement_id: string;
	hidden: boolean;
	collapsed: boolean;
	position: number | null;
}

export interface Viewer {
	id: string;
	role: string;
	locale: string;
	theme: string;
}

/** What a reference can reach (`$params`, `$data`, `$slot`, `$viewer`, `$actions`). */
export interface RefScope {
	params: Record<string, unknown>;
	data: Record<string, unknown>;
	slot: Record<string, unknown>;
	viewer: Record<string, unknown>;
	actions: Record<string, unknown>;
}

/** `GET /api/v1/slots` entry (`sv_layout::SlotDescriptor`). */
export interface SlotDescriptor {
	id: string;
	kind: 'single' | 'list' | 'keyed';
	context_schema: Record<string, string>;
	params_from_context: Record<string, string>;
	allowed_roots: string[];
	stack_children?: string[];
	allowed_actions: string[];
	max_entries: number;
	scope_kinds: string[];
}

export type ScopeKind = 'workspace' | 'project' | 'form';

/** `LayoutSummary` of the layout routes. */
export interface LayoutSummary {
	id: string;
	scope_kind: string;
	scope_id: string;
	layout_key: string;
	version: number;
	source: string;
	status: string;
	installation_id: string | null;
	generation: number | null;
	updated_at: string;
}

export interface LayoutRecord extends LayoutSummary {
	spec: LayoutSpec;
}

export interface LayoutValidationError {
	path: string;
	code: string;
	message?: string;
}

export interface LayoutVerdict {
	valid: boolean;
	errors: LayoutValidationError[];
}

export interface PlacementRecord {
	id: string;
	slot_id: string;
	scope_kind: string;
	scope_id: string;
	layout_id: string;
}
