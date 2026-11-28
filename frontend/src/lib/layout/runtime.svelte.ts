import { get } from 'svelte/store';
import { t as translate } from 'svelte-i18n';
import { formsApi } from '#lib/api/forms.js';
import { confirmAction } from '#lib/ui/dialog/confirm.js';
import { invokeMissing, layoutApi } from './api';
import { formById, loadSource, resolveFormId } from './data';
import { resolveDeep, resolveText, resolveValue } from './refs';
import type {
	ActionSpec,
	LayoutSpec,
	NavigateTarget,
	RefScope,
	SlotContextParams,
	Viewer
} from './types';

export type ActionStatus = 'idle' | 'running' | 'ok' | 'error';

export interface ActionState {
	status: ActionStatus;
	output?: unknown;
	error?: string;
}

/**
 * The plugin action endpoint is built by another lane. The first call that finds the route missing
 * flips this for the whole session, and every `invoke` button then renders disabled.
 */
const endpoint = $state({ missing: false });

export function invokeEndpointMissing(): boolean {
	return endpoint.missing;
}

export interface RuntimeOptions {
	spec: LayoutSpec;
	context: SlotContextParams;
	workspaceId: string;
	/** `$slot` as the host supplies it (the slot context, typed). */
	slot: Record<string, unknown>;
	params: Record<string, unknown>;
	viewer: Viewer;
	/** Package key to installation id, from the placements response. */
	installations: Record<string, string>;
	navigate: (path: string) => void;
}

/** State of one rendered layout: its data sources, action status and the action runner. */
export class LayoutRuntime {
	data = $state.raw<Record<string, unknown>>({});
	errors = $state.raw<Record<string, string>>({});
	loading = $state(true);
	actions = $state.raw<Record<string, ActionState>>({});

	#options: RuntimeOptions;
	#generation = 0;

	constructor(options: RuntimeOptions) {
		this.#options = options;
	}

	get spec(): LayoutSpec {
		return this.#options.spec;
	}

	get scope(): RefScope {
		return {
			params: this.#options.params,
			data: this.data,
			slot: this.#options.slot,
			viewer: { ...this.#options.viewer },
			actions: this.actions
		};
	}

	async load(): Promise<void> {
		const generation = ++this.#generation;
		const sources = Object.entries(this.#options.spec.data ?? {});
		const base: RefScope = { ...this.scope, data: {} };
		const results = await Promise.all(
			sources.map(async ([name, source]) => {
				try {
					return [name, await loadSource(source, base, this.#options.context.project_id)] as const;
				} catch {
					return [name, { ok: false, code: 'request_failed' } as const] as const;
				}
			})
		);
		// A newer load superseded this one.
		if (generation !== this.#generation) return;
		const data: Record<string, unknown> = {};
		const errors: Record<string, string> = {};
		for (const [name, result] of results) {
			if (result.ok) data[name] = result.value;
			else errors[name] = result.code;
		}
		this.data = data;
		this.errors = errors;
		this.loading = false;
	}

	/** Why an action cannot run now (shown as the disabled tooltip), or null when it can. */
	unavailable(name: string | undefined): string | null {
		if (!name) return null;
		const action = this.#options.spec.actions?.[name];
		if (!action) return get(translate)('layout.actionUnknown');
		if (action.kind !== 'invoke') return null;
		if (endpoint.missing || !this.#options.installations[action.package]) {
			return get(translate)('layout.notAvailable');
		}
		return null;
	}

	#set(name: string, state: ActionState): void {
		this.actions = { ...this.actions, [name]: state };
	}

	/** Runs a named action; `values` are the draft values of a `form` node for `submit`. */
	async run(name: string, values?: Record<string, unknown>): Promise<boolean> {
		const action = this.#options.spec.actions?.[name];
		if (!action || this.unavailable(name)) return false;
		if (!(await this.#confirm(action))) return false;
		this.#set(name, { status: 'running' });
		try {
			const outcome =
				action.kind === 'navigate'
					? await this.#navigate(action.to)
					: action.kind === 'submit'
						? await this.#submit(action, values ?? {})
						: await this.#invoke(action);
			this.#set(name, outcome);
			if (outcome.status === 'ok' && action.kind !== 'navigate') await this.load();
			return outcome.status === 'ok';
		} catch {
			this.#set(name, { status: 'error', error: 'request_failed' });
			return false;
		}
	}

	async #confirm(action: ActionSpec): Promise<boolean> {
		const t = get(translate);
		const locale = this.#options.viewer.locale;
		if (action.kind === 'navigate') return true;
		const author = action.confirm ? resolveText(action.confirm, this.scope, locale) : '';
		if (action.kind === 'submit' && !author) return true;
		return confirmAction({
			tone: 'neutral',
			title:
				action.kind === 'invoke'
					? t('layout.invokeConfirmTitle', {
							values: { action: action.export, package: action.package }
						})
					: author,
			body: action.kind === 'invoke' ? author || undefined : undefined,
			confirmLabel: t('layout.confirm')
		});
	}

	async #submit(
		action: Extract<ActionSpec, { kind: 'submit' }>,
		values: Record<string, unknown>
	): Promise<ActionState> {
		const source = this.#options.spec.data?.[action.data];
		if (!source || source.source !== 'form') return { status: 'error', error: 'action_unknown' };
		const scope = this.scope;
		const formId = await resolveFormId(source.form, scope, this.#options.context.project_id);
		if (!formId) return { status: 'error', error: 'binding_missing' };
		let result;
		if (action.mode === 'update') {
			const recordId = resolveValue(action.record, scope);
			if (typeof recordId !== 'string' || !recordId) {
				return { status: 'error', error: 'record_missing' };
			}
			result = await formsApi.updateRecord(recordId, { values });
		} else {
			result = await formsApi.createRecord(formId, { values });
		}
		return result.code === 0
			? { status: 'ok', output: result.data }
			: { status: 'error', error: result.error_code ?? 'request_failed' };
	}

	async #invoke(action: Extract<ActionSpec, { kind: 'invoke' }>): Promise<ActionState> {
		const installationId = this.#options.installations[action.package];
		if (!installationId) return { status: 'error', error: 'not_available' };
		const result = await layoutApi.invokeAction(installationId, action.export, {
			input: resolveDeep(action.input ?? {}, this.scope),
			context: this.#options.context
		});
		if (invokeMissing(result)) {
			endpoint.missing = true;
			return { status: 'error', error: 'not_available' };
		}
		return result.code === 0
			? { status: 'ok', output: result.data }
			: { status: 'error', error: result.error_code ?? 'request_failed' };
	}

	/**
	 * The forms route a navigation target lives under: the page's project, or, on a page without
	 * one (a workspace page), the project that owns the form the target names.
	 */
	async #formsBase(formId: string | null): Promise<string | null> {
		const { workspaceId, context } = this.#options;
		let projectId: string | null = context.project_id || null;
		if (!projectId && formId) projectId = (await formById(formId))?.project_id ?? null;
		return projectId ? `/workspace/${workspaceId}/projects/${projectId}/forms` : null;
	}

	async #navigate(to: NavigateTarget): Promise<ActionState> {
		const scope = this.scope;
		const { context } = this.#options;
		if ('record' in to) {
			const id = resolveValue(to.record.id, scope);
			if (typeof id !== 'string' || !id) return { status: 'error', error: 'record_missing' };
			let formId: string | null = null;
			if (!context.project_id) {
				const record = await formsApi.getRecord(id);
				formId = record.code === 0 && record.data ? record.data.form_id : null;
			}
			const base = await this.#formsBase(formId);
			if (!base) return { status: 'error', error: 'binding_missing' };
			this.#options.navigate(`${base}/records/${encodeURIComponent(id)}`);
		} else if ('form' in to) {
			const formId = await resolveFormId(to.form, scope, context.project_id);
			const base = formId ? await this.#formsBase(formId) : null;
			if (!formId || !base) return { status: 'error', error: 'binding_missing' };
			this.#options.navigate(`${base}?form=${encodeURIComponent(formId)}`);
		} else if ('view' in to) {
			const formId = await resolveFormId(to.view.form, scope, context.project_id);
			const base = formId ? await this.#formsBase(formId) : null;
			if (!formId || !base) return { status: 'error', error: 'binding_missing' };
			this.#options.navigate(
				`${base}?form=${encodeURIComponent(formId)}&view=${encodeURIComponent(to.view.view)}`
			);
		} else {
			// `route` is for sites only; the console never follows it.
			return { status: 'error', error: 'action_not_allowed' };
		}
		return { status: 'ok' };
	}
}
