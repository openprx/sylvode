// Flow Page -> Universal Forms record conversion wizard (v0.7 Forms Bridge;
// `contracts/ui-surface-v1.md` "后续版本 UI 派生" v0.7: "preview→commit 不可跳步";
// `versions/v0.7-forms-bridge.md` "Web 交互": source -> mapping -> preview -> commit).
//
// Rules this module owns, so they are unit-testable without a component:
// - steps cannot be skipped: `advance` moves exactly one step forward and only when
//   `canAdvance(step)` holds; `goTo` only moves backwards; leaving `mapping` always runs a server
//   preview first;
// - the source is read through `ObjectRepository.get` (no collaboration session); only `page`
//   objects can enter the wizard;
// - the mapping is the backend's real `form_record` shape (`apps/api/src/flow/bridge.rs`
//   `preview_conversion` / `execute_conversion`): `{target_form_id, title?, values}`, where
//   `values` are constant field values keyed by form field key. There is no block -> field
//   mapping on the server, so none is offered here;
// - a successful preview is frozen. Commit sends `preview_id`, `source_frontier` and
//   `target_schema_version` copied from that preview response, never from local state;
// - idempotency: one preview key per exact request (reused by a retry of that request, dropped
//   once it succeeded so an explicit re-preview is a new request); one commit key per
//   `preview_id`, reused by every retry against that preview;
// - an expired preview cannot be committed; `stale_frontier` offers "re-read the source and
//   preview again", which reads the object again and previews with a new key;
// - errors branch on `error_code` first and the numeric code second, never on `message`.

import type { ApiResult, PaginatedData } from '$lib/api/client';
import type {
	BridgePermissionState,
	FlowConversionJob,
	FlowConversionPreview,
	FlowObjectView
} from '$lib/api/flow';
import { formsApi, type FormField, type UniversalForm } from '$lib/api/forms';
import type { FlowCommandService } from './command-service';
import { flowErrorFromEnvelope, flowErrorI18nKey } from './errors';
import type { FlowObjectRepository } from './object-repository';
import {
	listWorkspaceProjects,
	newIdempotencyKey,
	type WorkspaceProjectOption
} from './package-export';

export type { BridgePermissionState, FlowConversionPreview, WorkspaceProjectOption };

export const CONVERT_STEPS = ['source', 'mapping', 'preview', 'commit'] as const;
export type ConvertStep = (typeof CONVERT_STEPS)[number];

export type SourceState = 'loading' | 'ready' | 'unsupported' | 'not_found' | 'failed';
export type FormsState = 'idle' | 'loading' | 'ready' | 'failed';
export type PreviewState = 'idle' | 'running' | 'ready' | 'failed';

/** How a form field can be filled by this wizard. `server` fields are assigned by Forms itself
 * (autonumber, formula); `unsupported` ones need an editor this wizard does not offer. */
export type FieldInputKind =
	| 'text'
	| 'long_text'
	| 'email'
	| 'date'
	| 'datetime'
	| 'integer'
	| 'rating'
	| 'progress'
	| 'decimal'
	| 'boolean'
	| 'single_select'
	| 'multi_select';

export type FieldDisposition =
	| { readonly kind: 'input'; readonly input: FieldInputKind }
	| { readonly kind: 'server' }
	| { readonly kind: 'unsupported' };

export interface MappingField {
	readonly key: string;
	readonly label: string;
	readonly type: string;
	readonly required: boolean;
	readonly options: readonly string[];
	readonly disposition: FieldDisposition;
}

/** Raw UI value of one field: a string (empty = not set) or, for multi-select, a string list. */
export type FieldInput = string | readonly string[];

export type FieldProblem =
	| 'required'
	| 'unmappable_required'
	| 'invalid_integer'
	| 'invalid_decimal'
	| 'out_of_range'
	| 'invalid_option'
	| 'invalid_boolean';

/** `form_record` mapping as `bridge.rs` reads it. */
export interface RecordConversionMapping {
	readonly target_form_id: string;
	readonly title?: string;
	readonly values: Readonly<Record<string, unknown>>;
}

export type ConvertFailureKind =
	| 'stale_frontier'
	| 'policy_rejected'
	| 'forbidden'
	| 'not_found'
	| 'unauthenticated'
	| 'rejected'
	| 'conflict'
	| 'unavailable'
	| 'flow_error';

export interface ConvertFailure {
	readonly kind: ConvertFailureKind;
	readonly messageKey: string;
	/** Whether the same request may be sent again under the same key. */
	readonly retryable: boolean;
	/** `policy_rejected.details.permission_state`, when the server attached one (ADR-0019). */
	readonly permissionState: BridgePermissionState | null;
}

// i18n keys, spelled out statically so the unit tests can check every one exists in zh and en.
export const STEP_KEYS: Readonly<Record<ConvertStep, string>> = {
	source: 'flow.bridge.convert.step.source',
	mapping: 'flow.bridge.convert.step.mapping',
	preview: 'flow.bridge.convert.step.preview',
	commit: 'flow.bridge.convert.step.commit'
};

export const FIELD_PROBLEM_KEYS: Readonly<Record<FieldProblem, string>> = {
	required: 'flow.bridge.convert.mapping.problem.required',
	unmappable_required: 'flow.bridge.convert.mapping.problem.unmappableRequired',
	invalid_integer: 'flow.bridge.convert.mapping.problem.invalidInteger',
	invalid_decimal: 'flow.bridge.convert.mapping.problem.invalidDecimal',
	out_of_range: 'flow.bridge.convert.mapping.problem.outOfRange',
	invalid_option: 'flow.bridge.convert.mapping.problem.invalidOption',
	invalid_boolean: 'flow.bridge.convert.mapping.problem.invalidBoolean'
};

export const FAILURE_KEYS = {
	stale_frontier: 'flow.error.stale_frontier',
	policy_rejected: 'flow.bridge.convert.error.policyRejected',
	forbidden: 'flow.error.forbidden',
	not_found: 'flow.bridge.convert.error.notFound',
	unauthenticated: 'flow.error.unauthenticated',
	rejected: 'flow.bridge.convert.error.rejected',
	conflict: 'flow.bridge.convert.error.conflict',
	unavailable: 'flow.bridge.convert.error.unavailable'
} as const;

/** ADR-0019 BR-2 action vocabulary, as the server emits it in `permission_state.actions`. */
export const PERMISSION_ACTION_KEYS: Readonly<Record<string, string>> = {
	'form.view': 'flow.bridge.convert.permission.action.formView',
	'record.create': 'flow.bridge.convert.permission.action.recordCreate',
	'record.update': 'flow.bridge.convert.permission.action.recordUpdate',
	'record.delete': 'flow.bridge.convert.permission.action.recordDelete',
	'record.export': 'flow.bridge.convert.permission.action.recordExport'
};

export const PERMISSION_ACCESS_KEYS: Readonly<Record<BridgePermissionState['access'], string>> = {
	controlled: 'flow.bridge.convert.permission.access.controlled',
	read_only: 'flow.bridge.convert.permission.access.readOnly'
};

export const PERMISSION_CONFIGURATION_KEYS: Readonly<
	Record<BridgePermissionState['configuration'], string>
> = {
	explicit: 'flow.bridge.convert.permission.configuration.explicit',
	unconfigured: 'flow.bridge.convert.permission.configuration.unconfigured'
};

export const PERMISSION_LIMIT_KEYS = {
	field_read_limited: 'flow.bridge.convert.permission.limit.fieldRead',
	field_write_limited: 'flow.bridge.convert.permission.limit.fieldWrite',
	record_limited: 'flow.bridge.convert.permission.limit.record'
} as const;

/** The i18n keys that explain a permission decision, in display order (ADR-0019: what the caller
 * can do, never why something else was denied). Unknown actions are left out. */
export function permissionExplanation(state: BridgePermissionState): string[] {
	const keys = [
		PERMISSION_ACCESS_KEYS[state.access],
		PERMISSION_CONFIGURATION_KEYS[state.configuration]
	];
	for (const action of state.actions) {
		const key = PERMISSION_ACTION_KEYS[action];
		if (key) keys.push(key);
	}
	if (state.field_read_limited) keys.push(PERMISSION_LIMIT_KEYS.field_read_limited);
	if (state.field_write_limited) keys.push(PERMISSION_LIMIT_KEYS.field_write_limited);
	if (state.record_limited) keys.push(PERMISSION_LIMIT_KEYS.record_limited);
	return keys.filter((key): key is string => typeof key === 'string');
}

const INPUT_KINDS: Readonly<Record<string, FieldInputKind>> = {
	text: 'text',
	phone: 'text',
	address: 'text',
	scan: 'text',
	textarea: 'long_text',
	rich_text: 'long_text',
	email: 'email',
	date: 'date',
	datetime: 'datetime',
	integer: 'integer',
	rating: 'rating',
	progress: 'progress',
	number: 'decimal',
	boolean: 'boolean',
	single_select: 'single_select',
	multi_select: 'multi_select'
};

/** Field types Forms fills in itself (`forms/native_create.rs`: autonumber after validation,
 * formula through the calculation hooks). */
const SERVER_FILLED = new Set(['autonumber', 'formula']);

export function fieldDisposition(type: string): FieldDisposition {
	const input = INPUT_KINDS[type];
	if (input) return { kind: 'input', input };
	if (SERVER_FILLED.has(type)) return { kind: 'server' };
	return { kind: 'unsupported' };
}

function asFieldList(schema: unknown): FormField[] {
	if (!schema || typeof schema !== 'object') return [];
	const fields = (schema as { fields?: unknown }).fields;
	if (!Array.isArray(fields)) return [];
	return fields.filter(
		(field): field is FormField =>
			!!field && typeof field === 'object' && typeof (field as FormField).key === 'string'
	);
}

/** The target form's fields, in schema order. */
export function describeFormFields(form: Pick<UniversalForm, 'schema'>): MappingField[] {
	return asFieldList(form.schema).map((field) => {
		const type = field.type ?? 'text';
		return {
			key: field.key,
			label: field.label?.trim() ? field.label : field.key,
			type,
			required: field.required === true,
			options: Array.isArray(field.options) ? field.options : [],
			disposition: fieldDisposition(type)
		};
	});
}

const INTEGER = /^-?\d{1,15}$/;
const DECIMAL = /^-?\d+(\.\d+)?$/;

function isUnset(input: FieldInput | undefined): boolean {
	if (input === undefined) return true;
	if (typeof input === 'string') return input.trim() === '';
	return input.length === 0;
}

function coerce(
	field: MappingField,
	kind: FieldInputKind,
	input: FieldInput
): { value: unknown } | { problem: FieldProblem } {
	if (kind === 'multi_select') {
		const items = typeof input === 'string' ? [input] : [...input];
		if (field.options.length > 0 && items.some((item) => !field.options.includes(item))) {
			return { problem: 'invalid_option' };
		}
		return { value: items };
	}
	const text = typeof input === 'string' ? input.trim() : '';
	switch (kind) {
		case 'integer':
		case 'rating':
		case 'progress': {
			if (!INTEGER.test(text)) return { problem: 'invalid_integer' };
			const value = Number(text);
			if (kind === 'rating' && (value < 1 || value > 5)) return { problem: 'out_of_range' };
			if (kind === 'progress' && (value < 0 || value > 100)) return { problem: 'out_of_range' };
			return { value };
		}
		case 'decimal':
			// `forms/values.rs::normalize_decimal_like` refuses a JSON number; it takes a string.
			return DECIMAL.test(text) ? { value: text } : { problem: 'invalid_decimal' };
		case 'boolean':
			if (text === 'true') return { value: true };
			if (text === 'false') return { value: false };
			return { problem: 'invalid_boolean' };
		case 'single_select':
			if (field.options.length > 0 && !field.options.includes(text)) {
				return { problem: 'invalid_option' };
			}
			return { value: text };
		default:
			return { value: text };
	}
}

/**
 * Builds the `form_record` mapping from the form's fields and the user's inputs. Unset inputs are
 * left out of `values`; a required field that is unset (or that this wizard cannot fill) is a
 * problem, since Forms would reject the record at commit. An empty title is left out so the
 * server uses the Page title.
 */
export function buildRecordMapping(
	formId: string,
	title: string,
	fields: readonly MappingField[],
	inputs: Readonly<Record<string, FieldInput>>
): { mapping: RecordConversionMapping; problems: Record<string, FieldProblem> } {
	const values: Record<string, unknown> = {};
	const problems: Record<string, FieldProblem> = {};
	for (const field of fields) {
		const disposition = field.disposition;
		if (disposition.kind === 'server') continue;
		if (disposition.kind === 'unsupported') {
			// `child_table` rows never live in `values`, so Forms does not require them there.
			if (field.required && field.type !== 'child_table') {
				problems[field.key] = 'unmappable_required';
			}
			continue;
		}
		const input = inputs[field.key];
		if (input === undefined || isUnset(input)) {
			if (field.required) problems[field.key] = 'required';
			continue;
		}
		const coerced = coerce(field, disposition.input, input);
		if ('problem' in coerced) problems[field.key] = coerced.problem;
		else values[field.key] = coerced.value;
	}
	const trimmed = title.trim();
	const mapping: RecordConversionMapping =
		trimmed === ''
			? { target_form_id: formId, values }
			: { target_form_id: formId, title: trimmed, values };
	return { mapping, problems };
}

function readPermissionState(details: unknown): BridgePermissionState | null {
	if (!details || typeof details !== 'object') return null;
	const state = (details as { permission_state?: unknown }).permission_state;
	if (!state || typeof state !== 'object') return null;
	const candidate = state as Partial<BridgePermissionState>;
	if (
		(candidate.access !== 'controlled' && candidate.access !== 'read_only') ||
		(candidate.configuration !== 'explicit' && candidate.configuration !== 'unconfigured') ||
		!Array.isArray(candidate.actions)
	) {
		return null;
	}
	return {
		access: candidate.access,
		configuration: candidate.configuration,
		actions: candidate.actions.filter((action): action is string => typeof action === 'string'),
		field_read_limited: candidate.field_read_limited === true,
		field_write_limited: candidate.field_write_limited === true,
		record_limited: candidate.record_limited === true
	};
}

function failure(
	kind: ConvertFailureKind,
	messageKey: string,
	retryable: boolean,
	permissionState: BridgePermissionState | null = null
): ConvertFailure {
	return { kind, messageKey, retryable, permissionState };
}

/**
 * Turns a non-success envelope into a wizard failure. `error_code` decides when present; legacy
 * envelopes without one are classified by their numeric code only. `message` is never read.
 */
export function classifyConvertFailure(
	result: Pick<ApiResult<unknown>, 'code' | 'error_code' | 'details'>
): ConvertFailure {
	const flowError = flowErrorFromEnvelope(result);
	if (flowError) {
		switch (flowError.code) {
			case 'stale_frontier':
				return failure('stale_frontier', FAILURE_KEYS.stale_frontier, false);
			case 'policy_rejected':
				return failure(
					'policy_rejected',
					FAILURE_KEYS.policy_rejected,
					false,
					readPermissionState(result.details)
				);
			case 'forbidden':
				return failure('forbidden', FAILURE_KEYS.forbidden, false);
			case 'not_found':
				return failure('not_found', FAILURE_KEYS.not_found, false);
			case 'unauthenticated':
				return failure('unauthenticated', FAILURE_KEYS.unauthenticated, false);
			default:
				return failure('flow_error', flowErrorI18nKey(flowError), flowError.recoverable);
		}
	}
	if (typeof result.error_code === 'string' && result.error_code !== '') {
		// A typed code this client does not know: permanent, no retry offered.
		return failure('rejected', FAILURE_KEYS.rejected, false);
	}
	switch (result.code) {
		case 401:
			return failure('unauthenticated', FAILURE_KEYS.unauthenticated, false);
		case 403:
			return failure('forbidden', FAILURE_KEYS.forbidden, false);
		case 404:
			return failure('not_found', FAILURE_KEYS.not_found, false);
		case 409:
			return failure('conflict', FAILURE_KEYS.conflict, false);
		default:
			return result.code >= 500
				? failure('unavailable', FAILURE_KEYS.unavailable, true)
				: failure('rejected', FAILURE_KEYS.rejected, false);
	}
}

function thrownFailure(): ConvertFailure {
	return failure('unavailable', FAILURE_KEYS.unavailable, true);
}

export interface ConvertSnapshot {
	readonly step: ConvertStep;
	readonly sourceState: SourceState;
	readonly source: FlowObjectView | null;
	readonly refreshingSource: boolean;
	readonly projects: readonly WorkspaceProjectOption[];
	readonly projectId: string | null;
	readonly formsState: FormsState;
	readonly forms: readonly UniversalForm[];
	readonly formId: string | null;
	readonly fields: readonly MappingField[];
	readonly title: string;
	readonly inputs: Readonly<Record<string, FieldInput>>;
	readonly problems: Readonly<Record<string, FieldProblem>>;
	readonly previewState: PreviewState;
	readonly preview: FlowConversionPreview | null;
	readonly previewFailure: ConvertFailure | null;
	readonly expired: boolean;
	readonly acknowledged: boolean;
	readonly committing: boolean;
	readonly commitFailure: ConvertFailure | null;
	readonly jobId: string | null;
	/** `canAdvance()` and `canCommit()` captured with the rest of the snapshot, so a component
	 * reading only the snapshot re-renders when they change. */
	readonly canAdvance: boolean;
	readonly canCommit: boolean;
}

export type ConvertCommitOutcome =
	| { readonly status: 'committed'; readonly jobId: string; readonly job: FlowConversionJob }
	| { readonly status: 'refused' }
	| { readonly status: 'failed'; readonly failure: ConvertFailure };

export interface ConvertWizardDeps {
	readonly repository: Pick<FlowObjectRepository, 'get'>;
	readonly commands: Pick<FlowCommandService, 'convertPreview' | 'convertCommit'>;
	readonly listProjects: (workspaceId: string) => Promise<WorkspaceProjectOption[]>;
	readonly listForms: (projectId: string) => Promise<ApiResult<PaginatedData<UniversalForm>>>;
	readonly newKey: () => string;
	readonly now: () => number;
}

function freeze<T extends object>(value: T): Readonly<T> {
	return Object.freeze({ ...value });
}

export class ConvertWizard {
	private readonly deps: ConvertWizardDeps;
	private currentStep: ConvertStep = 'source';
	private sourceState: SourceState = 'loading';
	private source: FlowObjectView | null = null;
	private refreshingSource = false;
	private projects: WorkspaceProjectOption[] = [];
	private projectId: string | null = null;
	private formsState: FormsState = 'idle';
	private forms: UniversalForm[] = [];
	private formId: string | null = null;
	private fields: MappingField[] = [];
	private title = '';
	private titleTouched = false;
	private inputs: Record<string, FieldInput> = {};
	private previewIntent: { key: string; signature: string } | null = null;
	private previewState: PreviewState = 'idle';
	private preview: Readonly<FlowConversionPreview> | null = null;
	private previewFailure: ConvertFailure | null = null;
	private acknowledged = false;
	private commitIntent: { key: string; previewId: string } | null = null;
	private committing = false;
	private commitFailure: ConvertFailure | null = null;
	private jobId: string | null = null;
	private formsRequest = 0;

	constructor(
		private readonly workspaceId: string,
		private readonly objectId: string,
		deps: Partial<ConvertWizardDeps> & Pick<ConvertWizardDeps, 'repository' | 'commands'>,
		private readonly onChange: (snapshot: ConvertSnapshot) => void = () => {}
	) {
		this.deps = {
			repository: deps.repository,
			commands: deps.commands,
			listProjects: deps.listProjects ?? listWorkspaceProjects,
			listForms: deps.listForms ?? ((projectId) => formsApi.list(projectId, { per_page: 100 })),
			newKey: deps.newKey ?? newIdempotencyKey,
			now: deps.now ?? Date.now
		};
	}

	get step(): ConvertStep {
		return this.currentStep;
	}

	snapshot(): ConvertSnapshot {
		return {
			step: this.currentStep,
			sourceState: this.sourceState,
			source: this.source,
			refreshingSource: this.refreshingSource,
			projects: this.projects,
			projectId: this.projectId,
			formsState: this.formsState,
			forms: this.forms,
			formId: this.formId,
			fields: this.fields,
			title: this.title,
			inputs: { ...this.inputs },
			problems: this.currentMapping()?.problems ?? {},
			previewState: this.previewState,
			preview: this.preview,
			previewFailure: this.previewFailure,
			expired: this.isExpired(),
			acknowledged: this.acknowledged,
			committing: this.committing,
			commitFailure: this.commitFailure,
			jobId: this.jobId,
			canAdvance: this.canAdvance(),
			canCommit: this.canCommit()
		};
	}

	/** Reads the source object (no collaboration session) and, for a Page, the project list. */
	async load(): Promise<SourceState> {
		this.sourceState = 'loading';
		this.emit();
		const state = await this.readSource();
		if (state === 'ready') {
			this.projects = await this.deps.listProjects(this.workspaceId);
		}
		this.emit();
		return state;
	}

	private async readSource(): Promise<SourceState> {
		let result: ApiResult<FlowObjectView>;
		try {
			result = await this.deps.repository.get(this.objectId);
		} catch {
			// A transport failure while re-reading keeps the source already shown.
			if (!this.source) this.sourceState = 'failed';
			return 'failed';
		}
		if (result.code !== 0 || !result.data) {
			const failure = classifyConvertFailure(result);
			if (failure.kind === 'forbidden' || failure.kind === 'not_found') {
				// Forbidden-safe: a hidden object and a missing one look the same.
				this.source = null;
				this.sourceState = 'not_found';
				return 'not_found';
			}
			if (!this.source) this.sourceState = 'failed';
			return 'failed';
		}
		const object = result.data;
		if (object.object_type !== 'page') {
			this.source = null;
			this.sourceState = 'unsupported';
			return 'unsupported';
		}
		this.source = freeze(object);
		this.sourceState = 'ready';
		if (!this.titleTouched) this.title = object.title;
		return 'ready';
	}

	/** Whether the wizard may move past `step`. */
	canAdvance(step: ConvertStep = this.currentStep): boolean {
		switch (step) {
			case 'source':
				return this.sourceState === 'ready' && this.source !== null;
			case 'mapping': {
				const built = this.currentMapping();
				return (
					this.canAdvance('source') && built !== null && Object.keys(built.problems).length === 0
				);
			}
			case 'preview':
				return (
					this.canAdvance('mapping') &&
					this.preview !== null &&
					this.previewFailure === null &&
					!this.isExpired()
				);
			case 'commit':
				return false;
		}
	}

	/** Moves exactly one step forward. Leaving `mapping` runs the server preview first and always
	 * lands on `preview`, where its result or error is shown. */
	async advance(): Promise<boolean> {
		if (this.previewState === 'running' || this.committing) return false;
		const index = CONVERT_STEPS.indexOf(this.currentStep);
		if (index >= CONVERT_STEPS.length - 1 || !this.canAdvance()) return false;
		if (this.currentStep === 'mapping' && this.preview === null) {
			this.currentStep = 'preview';
			await this.runPreview();
			return true;
		}
		this.currentStep = CONVERT_STEPS[index + 1];
		this.emit();
		return true;
	}

	/** Backwards navigation only; entered values are kept. */
	goTo(step: ConvertStep): boolean {
		if (CONVERT_STEPS.indexOf(step) > CONVERT_STEPS.indexOf(this.currentStep)) return false;
		if (this.previewState === 'running' || this.committing) return false;
		this.currentStep = step;
		this.emit();
		return true;
	}

	back(): boolean {
		const index = CONVERT_STEPS.indexOf(this.currentStep);
		return index > 0 ? this.goTo(CONVERT_STEPS[index - 1]) : false;
	}

	async selectProject(projectId: string | null): Promise<void> {
		if (!this.editable() || projectId === this.projectId) return;
		this.projectId = projectId;
		this.formId = null;
		this.fields = [];
		this.inputs = {};
		this.forms = [];
		this.invalidatePreview();
		const request = ++this.formsRequest;
		if (projectId === null) {
			this.formsState = 'idle';
			this.emit();
			return;
		}
		this.formsState = 'loading';
		this.emit();
		let result: ApiResult<PaginatedData<UniversalForm>>;
		try {
			result = await this.deps.listForms(projectId);
		} catch {
			if (request !== this.formsRequest) return;
			this.formsState = 'failed';
			this.emit();
			return;
		}
		if (request !== this.formsRequest) return;
		if (result.code !== 0 || !result.data) {
			this.formsState = 'failed';
		} else {
			this.forms = result.data.items.filter((form) => !form.archived_at);
			this.formsState = 'ready';
		}
		this.emit();
	}

	selectForm(formId: string | null): void {
		if (!this.editable() || formId === this.formId) return;
		const form = formId === null ? null : this.forms.find((item) => item.id === formId);
		if (formId !== null && !form) return;
		this.formId = form ? form.id : null;
		this.fields = form ? describeFormFields(form) : [];
		this.inputs = {};
		this.invalidatePreview();
		this.emit();
	}

	setTitle(title: string): void {
		if (!this.editable() || title === this.title) return;
		this.title = title;
		this.titleTouched = true;
		this.invalidatePreview();
		this.emit();
	}

	setValue(key: string, value: FieldInput): void {
		if (!this.editable() || !this.fields.some((field) => field.key === key)) return;
		this.inputs = { ...this.inputs, [key]: typeof value === 'string' ? value : [...value] };
		this.invalidatePreview();
		this.emit();
	}

	/** Runs (or retries) the preview for the current source frontier and mapping. */
	async runPreview(): Promise<boolean> {
		if (this.previewState === 'running' || this.committing) return false;
		if (this.currentStep !== 'preview') return false;
		const source = this.source;
		const built = this.currentMapping();
		if (!source || !built || Object.keys(built.problems).length > 0) return false;
		const signature = JSON.stringify([source.id, source.frontier, built.mapping]);
		if (!this.previewIntent || this.previewIntent.signature !== signature) {
			this.previewIntent = { key: this.deps.newKey(), signature };
		}
		const intent = this.previewIntent;
		this.clearPreviewResult();
		this.previewState = 'running';
		this.emit();
		let result: ApiResult<FlowConversionPreview>;
		try {
			result = await this.deps.commands.convertPreview({
				source_object_id: source.id,
				source_frontier: source.frontier,
				target_type: 'form_record',
				mapping: { ...built.mapping },
				idempotency_key: intent.key
			});
		} catch {
			this.previewState = 'failed';
			this.previewFailure = thrownFailure();
			this.emit();
			return false;
		}
		if (result.code === 0 && result.data) {
			this.preview = freeze(result.data);
			this.previewState = 'ready';
			// This request is done; an explicit re-preview (e.g. after expiry) is a new request.
			this.previewIntent = null;
			this.emit();
			return true;
		}
		const failure = classifyConvertFailure(result);
		this.previewFailure = failure;
		this.previewState = 'failed';
		if (!failure.retryable) this.previewIntent = null;
		this.emit();
		return false;
	}

	/** `stale_frontier` recovery: read the source again (new frontier) and preview with a new key.
	 * Always lands on the preview step. */
	async refreshSourceAndPreview(): Promise<boolean> {
		if (this.previewState === 'running' || this.committing || this.refreshingSource) return false;
		const stale =
			this.previewFailure?.kind === 'stale_frontier' ||
			this.commitFailure?.kind === 'stale_frontier';
		if (!stale) return false;
		this.refreshingSource = true;
		this.emit();
		const state = await this.readSource();
		this.refreshingSource = false;
		this.invalidatePreview();
		this.previewIntent = null;
		if (state !== 'ready') {
			this.emit();
			return false;
		}
		this.currentStep = 'preview';
		return this.runPreview();
	}

	acknowledge(value: boolean): void {
		if (this.committing || this.jobId !== null) return;
		this.acknowledged = value && this.preview !== null;
		this.emit();
	}

	isExpired(): boolean {
		if (!this.preview) return false;
		const expiresAt = Date.parse(this.preview.expires_at);
		return !Number.isFinite(expiresAt) || this.deps.now() >= expiresAt;
	}

	secondsUntilExpiry(): number | null {
		if (!this.preview) return null;
		const expiresAt = Date.parse(this.preview.expires_at);
		if (!Number.isFinite(expiresAt)) return 0;
		return Math.max(0, Math.floor((expiresAt - this.deps.now()) / 1000));
	}

	canCommit(): boolean {
		return (
			this.currentStep === 'commit' &&
			this.canAdvance('preview') &&
			this.acknowledged &&
			!this.committing &&
			this.jobId === null &&
			(this.commitFailure === null || this.commitFailure.retryable)
		);
	}

	/** Commits the frozen preview. The three preview-derived fields are copied from the preview
	 * response; the key is fixed per `preview_id`, so a retry resends it. */
	async commit(): Promise<ConvertCommitOutcome> {
		const preview = this.preview;
		if (!preview || !this.canCommit()) return { status: 'refused' };
		if (!this.commitIntent || this.commitIntent.previewId !== preview.preview_id) {
			this.commitIntent = { key: this.deps.newKey(), previewId: preview.preview_id };
		}
		const key = this.commitIntent.key;
		this.committing = true;
		this.commitFailure = null;
		this.emit();
		let result: ApiResult<FlowConversionJob>;
		try {
			result = await this.deps.commands.convertCommit({
				preview_id: preview.preview_id,
				source_frontier: preview.source_frontier,
				target_schema_version: preview.target_schema_version,
				idempotency_key: key,
				confirm: true
			});
		} catch {
			this.committing = false;
			this.commitFailure = thrownFailure();
			this.emit();
			return { status: 'failed', failure: this.commitFailure };
		}
		this.committing = false;
		if (result.code === 0 && result.data && typeof result.data.job_id === 'string') {
			this.jobId = result.data.job_id;
			this.emit();
			return { status: 'committed', jobId: result.data.job_id, job: result.data };
		}
		const failure = classifyConvertFailure(result);
		this.commitFailure = failure;
		this.emit();
		return { status: 'failed', failure };
	}

	private editable(): boolean {
		return (
			this.currentStep === 'mapping' &&
			this.previewState !== 'running' &&
			!this.committing &&
			this.jobId === null
		);
	}

	private currentMapping(): ReturnType<typeof buildRecordMapping> | null {
		if (!this.formId) return null;
		return buildRecordMapping(this.formId, this.title, this.fields, this.inputs);
	}

	private clearPreviewResult(): void {
		this.preview = null;
		this.previewFailure = null;
		this.acknowledged = false;
		this.commitFailure = null;
	}

	private invalidatePreview(): void {
		this.clearPreviewResult();
		this.previewState = 'idle';
	}

	private emit(): void {
		this.onChange(this.snapshot());
	}
}
