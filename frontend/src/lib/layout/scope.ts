import { get } from 'svelte/store';
import { locale } from 'svelte-i18n';
import { authStore } from '#lib/stores/auth.js';
import { resolvedTheme } from '#lib/theme.js';
import type { LayoutSpec, RefScope, SlotContextParams, Viewer } from './types';

/** `$viewer` / `viewer` of the signed-in user. */
export function currentViewer(): Viewer {
	const user = get(authStore).user;
	return {
		id: user?.id ?? '',
		role: user?.role ?? 'member',
		locale: get(locale) ?? 'en',
		theme: get(resolvedTheme)
	};
}

/** `$slot` as the host supplies it: the typed slot context. */
export function slotValues(context: SlotContextParams): Record<string, unknown> {
	return { ...context };
}

/** The layout parameters the slot context fills (a parameter named like a context member). */
export function paramsFromSlot(
	spec: Pick<LayoutSpec, 'params'>,
	slot: Record<string, unknown>
): Record<string, unknown> {
	const params: Record<string, unknown> = {};
	for (const name of Object.keys(spec.params ?? {})) {
		if (name in slot) params[name] = slot[name];
	}
	return params;
}

/** The scope a placement `condition` sees: no data sources are loaded yet at that point. */
export function conditionScope(
	spec: Pick<LayoutSpec, 'params'>,
	context: SlotContextParams
): Pick<RefScope, 'params' | 'data' | 'slot' | 'viewer'> {
	const slot = slotValues(context);
	return { params: paramsFromSlot(spec, slot), data: {}, slot, viewer: { ...currentViewer() } };
}
