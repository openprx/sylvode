/**
 * View model for the "Flow" section of the Forms record detail page.
 *
 * The record detail (`GET /form-records/{record_id}`) carries `flow_lineage`: the live Flow
 * references that point at the record and the conversion jobs that created it
 * (`versions/v0.7-forms-bridge.md` "Web 交互": lineage is visible from both ends). This module
 * turns that field into the rows the page renders, so the visibility rules can be tested
 * without a browser:
 *
 * - the section is hidden when the field is missing (older API) or both lists are empty
 *   (Flow or the bridge is off, or nothing refers to the record);
 * - a row whose source the caller cannot read (`source_available: false`) carries no link, no
 *   title, no mode, no status and no time -- only its id for keying and the placeholder;
 * - a readable source links to its Page (`/workspace/{ws}/flow/{sourceObjectId}`);
 * - a conversion links to its job page (`/workspace/{ws}/flow/conversions/{jobId}`) only when
 *   `job_available` is true (the job endpoint answers `not_found` to anyone else, so a link
 *   would lead to an error page).
 *
 * The page builds both links with `resolve()` from these ids; a row without an id has no link.
 *
 * The section is read-only: references are removed from the Flow side (the Page's card), and
 * removing one never deletes the record.
 */

import type {
	FormRecordFlowLineage,
	FormRecordLineageConversion,
	FormRecordLineageReference
} from '$lib/api/forms';

export type LineageDisplayMode = 'reference' | 'embed' | 'unknown';
export type LineageJobStatus = 'started' | 'completed' | 'failed' | 'unknown';

export type FlowLineageReferenceRow =
	| {
			readonly key: string;
			readonly available: true;
			/** The source Page's object id; the page links it. */
			readonly sourceObjectId: string;
			/** `null` when the source has no projection row yet; the page shows "Untitled". */
			readonly sourceTitle: string | null;
			readonly mode: LineageDisplayMode;
			readonly createdAt: string;
	  }
	| { readonly key: string; readonly available: false };

export type FlowLineageConversionRow =
	| {
			readonly key: string;
			readonly available: true;
			readonly sourceObjectId: string;
			readonly sourceTitle: string | null;
			readonly status: LineageJobStatus;
			/** The job id to link, `null` when the caller may not open the job. */
			readonly linkedJobId: string | null;
			readonly createdAt: string;
	  }
	| { readonly key: string; readonly available: false };

export interface FlowLineageView {
	readonly references: readonly FlowLineageReferenceRow[];
	readonly conversions: readonly FlowLineageConversionRow[];
}

function nonEmpty(value: unknown): string | null {
	return typeof value === 'string' && value.trim().length > 0 ? value : null;
}

function displayMode(display: unknown): LineageDisplayMode {
	if (!display || typeof display !== 'object') return 'unknown';
	const mode = (display as Record<string, unknown>).mode;
	return mode === 'reference' || mode === 'embed' ? mode : 'unknown';
}

function jobStatus(status: unknown): LineageJobStatus {
	return status === 'started' || status === 'completed' || status === 'failed' ? status : 'unknown';
}

/** Only `source_available === true` with a usable object id counts as readable; anything else
 * (false, missing, malformed) is treated as unavailable so nothing about the source leaks. */
function readableSource(entry: {
	source_available?: unknown;
	source_object_id?: unknown;
}): string | null {
	return entry.source_available === true ? nonEmpty(entry.source_object_id) : null;
}

function referenceRow(entry: FormRecordLineageReference): FlowLineageReferenceRow {
	const key = String(entry.reference_id);
	const objectId = readableSource(entry);
	if (objectId === null || entry.source_available !== true) return { key, available: false };
	return {
		key,
		available: true,
		sourceObjectId: objectId,
		sourceTitle: nonEmpty(entry.source_title),
		mode: displayMode(entry.display),
		createdAt: entry.created_at
	};
}

function conversionRow(entry: FormRecordLineageConversion): FlowLineageConversionRow {
	const key = String(entry.job_id);
	const objectId = readableSource(entry);
	if (objectId === null || entry.source_available !== true) return { key, available: false };
	return {
		key,
		available: true,
		sourceObjectId: objectId,
		sourceTitle: nonEmpty(entry.source_title),
		status: jobStatus(entry.status),
		linkedJobId: entry.job_available === true ? key : null,
		createdAt: entry.created_at
	};
}

/** The rows of the record detail's Flow section, or `null` when the section must not render. */
export function flowLineageView(
	lineage: FormRecordFlowLineage | null | undefined
): FlowLineageView | null {
	if (!lineage || typeof lineage !== 'object') return null;
	const references = Array.isArray(lineage.references) ? lineage.references : [];
	const conversions = Array.isArray(lineage.conversions) ? lineage.conversions : [];
	if (references.length === 0 && conversions.length === 0) return null;
	return {
		references: references.map(referenceRow),
		conversions: conversions.map(conversionRow)
	};
}
