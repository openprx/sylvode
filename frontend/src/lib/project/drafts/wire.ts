/** The draft API of `apps/api/src/drafts` (contract C3, section 7). */

export type DraftKind =
	| 'form_schema'
	| 'layout'
	| 'package_install'
	| 'site_page'
	| 'package_upload';
export type DraftStatus = 'pending' | 'approved' | 'rejected' | 'superseded' | 'failed';

export interface DraftAuthor {
	kind: string;
	id: string;
	name: string | null;
}

/** The plugin call that wrote a draft; the author is then the person who invoked the plugin. */
export interface DraftSource {
	installation_id: string;
	generation: number;
	package_key: string;
}

export interface DraftSummary {
	id: string;
	kind: DraftKind;
	/** Null for a package upload, which belongs to the workspace. */
	project_id: string | null;
	target: Record<string, unknown>;
	author: DraftAuthor;
	status: DraftStatus;
	valid: boolean;
	created_at: string;
	/** Set when a plugin call wrote the draft. */
	source?: DraftSource | null;
}

export interface ValidationIssue {
	path?: string;
	code: string;
	message?: string;
}

export interface DraftValidation {
	valid: boolean;
	errors: ValidationIssue[];
	warnings?: ValidationIssue[];
}

export interface CapabilityDiffItem {
	capability: string;
	selector: Record<string, unknown>;
	risk: string;
	new: boolean;
	paths?: string[];
}

export interface Draft extends DraftSummary {
	spec: unknown;
	note: string | null;
	validation: DraftValidation;
	capability_diff: CapabilityDiffItem[];
	/** Pages of published sites that would show the artefact; empty until sites exist. */
	public: Record<string, unknown>[];
	review_digest: string;
	review: { by: string | null; at: string | null; comment: string | null } | null;
	result: Record<string, unknown> | null;
}
