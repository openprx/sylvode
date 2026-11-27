import type { LayoutValidationError, ScopeKind, SlotContextParams, SlotDescriptor } from '../types';

/** A scope the viewer may offer a placement at, with the id the server resolves it by. */
export interface OfferedScope {
	kind: ScopeKind;
	id: string;
}

/**
 * The scopes the "Create here" panel offers for a slot and its context. The server decides every
 * write (`require_write`): workspace scope is for owners and admins, project scope for members,
 * form scope needs `form.design`. This only keeps the panel from offering what the slot or the
 * role can never take; a refusal that still comes back is shown as it is.
 */
export function offeredScopes(
	slot: Pick<SlotDescriptor, 'scope_kinds'>,
	context: SlotContextParams,
	workspaceId: string,
	role: string | null | undefined
): OfferedScope[] {
	if (!role) return [];
	const scopes: OfferedScope[] = [];
	if (slot.scope_kinds.includes('form') && context.form_id) {
		scopes.push({ kind: 'form', id: context.form_id });
	}
	if (slot.scope_kinds.includes('project')) {
		scopes.push({ kind: 'project', id: context.project_id });
	}
	if (slot.scope_kinds.includes('workspace') && (role === 'owner' || role === 'admin')) {
		scopes.push({ kind: 'workspace', id: workspaceId });
	}
	return scopes;
}

/** A layout spec that validates in the slot, as the starting text of the JSON editor. */
export function starterSpec(slot: Pick<SlotDescriptor, 'allowed_roots'>, suffix: string): object {
	const base = {
		schema: 'sv.layout.v1',
		key: `custom.layout_${suffix}`,
		title: { en: 'New layout' }
	};
	const roots = slot.allowed_roots;
	if (roots.includes('card')) {
		return {
			...base,
			root: {
				type: 'card',
				props: { title: { en: 'New layout' } },
				children: [{ type: 'text', props: { value: { en: 'Edit this text.' } } }]
			}
		};
	}
	if (roots.includes('stack')) {
		return {
			...base,
			root: {
				type: 'stack',
				props: { direction: 'horizontal' },
				children: [{ type: 'button', props: { label: { en: 'Button' } } }]
			}
		};
	}
	if (roots.includes('button')) {
		return { ...base, root: { type: 'button', props: { label: { en: 'Button' } } } };
	}
	return { ...base, root: { type: roots[0] ?? 'text', props: { value: { en: 'Text' } } } };
}

export type ParsedSpec = { ok: true; spec: Record<string, unknown> } | { ok: false };

/** The editor text as an object; anything but a JSON object is not a layout. */
export function parseSpec(text: string): ParsedSpec {
	try {
		const value: unknown = JSON.parse(text);
		if (value !== null && typeof value === 'object' && !Array.isArray(value)) {
			return { ok: true, spec: value as Record<string, unknown> };
		}
	} catch {
		// Not JSON: the caller shows the syntax message.
	}
	return { ok: false };
}

/** One line per server error: the node path, the code and the detail. */
export function errorLines(errors: readonly LayoutValidationError[]): string[] {
	return errors.map((error) => {
		const path = error.path === '' ? '/' : error.path;
		return `${path}  ${error.code}${error.message ? `  ${error.message}` : ''}`;
	});
}

/** Errors of a refused save (`details.errors` of `layout_invalid`, `cel_*` conditions) as lines. */
export function refusalLines(result: {
	error_code?: string;
	message?: string;
	details?: unknown;
}): string[] {
	const details = result.details;
	if (details && typeof details === 'object' && 'errors' in details) {
		const errors = (details as { errors: unknown }).errors;
		if (Array.isArray(errors)) {
			return errorLines(
				errors.filter(
					(item): item is LayoutValidationError =>
						!!item &&
						typeof item === 'object' &&
						typeof (item as LayoutValidationError).path === 'string' &&
						typeof (item as LayoutValidationError).code === 'string'
				)
			);
		}
	}
	return [[result.error_code, result.message].filter(Boolean).join('  ')];
}

/** Layouts a user can place from a scope: active, written by people (not an install package). */
export function placeable<T extends { status: string; installation_id: string | null }>(
	items: readonly T[]
): T[] {
	return items.filter((item) => item.status === 'active' && item.installation_id === null);
}
