import { formsApi, type FormRecord, type UniversalForm } from '#lib/api/forms.js';
import { parseFields } from '#lib/forms/designer-schema.js';
import { displayValue, resolveDeep, resolveValue } from './refs';
import type { DataSourceSpec, FilterSpec, RefScope } from './types';

export type SourceResult = { ok: true; value: unknown } | { ok: false; code: string };

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SERVER_OPS = new Set(['eq', 'neq', 'contains', 'gt', 'gte', 'lt', 'lte', 'not_empty']);

const formsByProject = new Map<string, Promise<UniversalForm[]>>();
const formsById = new Map<string, Promise<UniversalForm | null>>();

function projectForms(projectId: string): Promise<UniversalForm[]> {
	let cached = formsByProject.get(projectId);
	if (!cached) {
		cached = formsApi
			.list(projectId, { per_page: 100 })
			.then((result) => (result.code === 0 && result.data ? result.data.items : []));
		formsByProject.set(projectId, cached);
		// A failed read must not stick: the next render asks again.
		void cached.then((items) => {
			if (items.length === 0) formsByProject.delete(projectId);
		});
	}
	return cached;
}

export function formById(formId: string): Promise<UniversalForm | null> {
	let cached = formsById.get(formId);
	if (!cached) {
		cached = formsApi.get(formId).then((result) => (result.code === 0 ? result.data : null));
		formsById.set(formId, cached);
		void cached.then((form) => {
			if (!form) formsById.delete(formId);
		});
	}
	return cached;
}

/** `form:<key>` of the layout's project, or an id; anything else (an unresolved `$bind`) is refused. */
export async function resolveFormId(
	raw: unknown,
	scope: RefScope,
	projectId: string
): Promise<string | null> {
	const value = resolveValue(raw, scope);
	if (typeof value !== 'string') return null;
	if (value.startsWith('form:')) {
		const key = value.slice(5);
		// A page without a project (a workspace page) binds forms by id; a key is ambiguous there.
		if (!projectId) return null;
		const forms = await projectForms(projectId);
		return forms.find((form) => form.key === key)?.id ?? null;
	}
	return UUID.test(value) ? value : null;
}

function pick<T extends { key: string }>(items: T[], keys: string[] | undefined): T[] {
	return keys ? items.filter((item) => keys.includes(item.key)) : items;
}

function pickValues(values: Record<string, unknown>, keys: string[] | undefined) {
	if (!keys) return values;
	return Object.fromEntries(Object.entries(values).filter(([key]) => keys.includes(key)));
}

function recordRow(record: FormRecord, keys: string[] | undefined) {
	return {
		id: record.id,
		title: record.title,
		values: pickValues(record.values, keys),
		created_at: record.created_at,
		updated_at: record.updated_at
	};
}

function matchesFilter(record: FormRecord, filter: FilterSpec, value: unknown): boolean {
	const actual = record.values[filter.field];
	const a = displayValue(actual);
	const b = displayValue(value);
	switch (filter.op) {
		case 'eq':
			return a === b;
		case 'neq':
			return a !== b;
		case 'contains':
			return a.toLowerCase().includes(b.toLowerCase());
		case 'not_empty':
			return a !== '';
		case 'gt':
			return Number(a) > Number(b);
		case 'gte':
			return Number(a) >= Number(b);
		case 'lt':
			return Number(a) < Number(b);
		case 'lte':
			return Number(a) <= Number(b);
		default:
			return true;
	}
}

function fail(code: string): SourceResult {
	return { ok: false, code };
}

/** Reads one data source with the viewer's own session; the server decides what is visible. */
export async function loadSource(
	spec: DataSourceSpec,
	scope: RefScope,
	projectId: string
): Promise<SourceResult> {
	switch (spec.source) {
		case 'record': {
			const recordId = resolveValue(spec.record, scope);
			if (typeof recordId !== 'string' || !recordId) return fail('record_missing');
			const result = await formsApi.getRecord(recordId);
			if (result.code !== 0 || !result.data) return fail(result.error_code ?? 'not_found');
			const record = result.data;
			if (spec.form) {
				const formId = await resolveFormId(spec.form, scope, projectId);
				if (formId !== record.form_id) return fail('not_found');
			}
			const form = await formById(record.form_id);
			const fields = pick(form ? parseFields(form.schema) : [], spec.fields)
				.filter((field) => field.key in record.values)
				.map((field) => ({ key: field.key, label: field.label ?? field.key, type: field.type }));
			return { ok: true, value: { ...recordRow(record, spec.fields), fields } };
		}
		case 'query': {
			const formId = await resolveFormId(spec.form, scope, projectId);
			if (!formId) return fail('binding_missing');
			const filters = (spec.filter ?? []).map((filter) => ({
				filter,
				value: resolveDeep(filter.value, scope)
			}));
			const [first, ...rest] = filters;
			const sort = spec.sort?.[0];
			const result = await formsApi.listRecords(formId, {
				per_page: Math.min(spec.page_size ?? 20, 200),
				...(sort ? { sort: `${sort.field}:${sort.dir ?? 'asc'}` } : {}),
				...(first && SERVER_OPS.has(first.filter.op)
					? {
							filter_field: first.filter.field,
							filter_op: first.filter.op,
							filter_value: displayValue(first.value)
						}
					: {})
			});
			if (result.code !== 0 || !result.data) return fail(result.error_code ?? 'not_found');
			// The list API takes one filter; the rest narrow the page on the client.
			const items = result.data.items.filter((record) =>
				rest.every(({ filter, value }) => matchesFilter(record, filter, value))
			);
			return {
				ok: true,
				value: {
					items: items.map((record) => recordRow(record, spec.fields)),
					total: rest.length ? items.length : result.data.total
				}
			};
		}
		case 'form': {
			const formId = await resolveFormId(spec.form, scope, projectId);
			if (!formId) return fail('binding_missing');
			const form = await formById(formId);
			if (!form) return fail('not_found');
			const fields = pick(parseFields(form.schema), spec.fields).map((field) => ({
				key: field.key,
				label: field.label ?? field.key,
				type: field.type,
				required: Boolean(field.required),
				read_only: field.read_only === true,
				...(field.options?.length ? { options: field.options } : {})
			}));
			return { ok: true, value: { form_id: formId, fields } };
		}
		default:
			return fail('source_not_available');
	}
}
