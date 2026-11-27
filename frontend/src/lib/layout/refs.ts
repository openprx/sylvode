import type { I18nText, RefScope, TextValue } from './types';

const REF = /^\$(params|data|slot|viewer|actions)((?:\.[a-z_][a-z0-9_]*|\[[0-9]{1,4}\])*)$/;

export function isRef(value: unknown): value is string {
	return typeof value === 'string' && REF.test(value);
}

function segments(path: string): (string | number)[] {
	const out: (string | number)[] = [];
	for (const match of path.matchAll(/\.([a-z_][a-z0-9_]*)|\[([0-9]{1,4})\]/g)) {
		out.push(match[1] !== undefined ? match[1] : Number(match[2]));
	}
	return out;
}

/** Resolves one reference; a missing path yields `undefined` (renders empty). */
export function resolveRef(ref: string, scope: RefScope): unknown {
	const match = REF.exec(ref);
	if (!match) return undefined;
	let current: unknown = scope[match[1] as keyof RefScope];
	for (const segment of segments(match[2])) {
		if (current === null || typeof current !== 'object') return undefined;
		current = (current as Record<string | number, unknown>)[segment];
	}
	return current;
}

/** A value that may be a reference: resolves it, otherwise returns it unchanged. */
export function resolveValue(value: unknown, scope: RefScope): unknown {
	return isRef(value) ? resolveRef(value, scope) : value;
}

/** Deep-resolves references inside an input object (action inputs, filter values). */
export function resolveDeep(value: unknown, scope: RefScope): unknown {
	if (isRef(value)) return resolveRef(value, scope);
	if (Array.isArray(value)) return value.map((item) => resolveDeep(item, scope));
	if (value !== null && typeof value === 'object') {
		return Object.fromEntries(
			Object.entries(value as Record<string, unknown>).map(([key, item]) => [
				key,
				resolveDeep(item, scope)
			])
		);
	}
	return value;
}

function isI18nMap(value: unknown): value is I18nText {
	return (
		!(value !== null && typeof value === 'object' && 'cel' in value) &&
		value !== null &&
		typeof value === 'object' &&
		!Array.isArray(value) &&
		Object.values(value as object).every((item) => typeof item === 'string')
	);
}

/** Display string of any resolved value. */
export function displayValue(value: unknown): string {
	if (value === null || value === undefined) return '';
	if (typeof value === 'string') return value;
	if (typeof value === 'number' || typeof value === 'boolean') return String(value);
	if (Array.isArray(value)) return value.map(displayValue).join(', ');
	return JSON.stringify(value);
}

/**
 * Text of a literal, an i18n map (locale, then `en`, then any entry), or a reference. A computed
 * `{cel}` value is shown empty until the shared rules mirror is available (see `when.ts`).
 */
export function resolveText(value: TextValue | unknown, scope: RefScope, locale: string): string {
	if (typeof value === 'string') return displayValue(resolveValue(value, scope));
	if (isI18nMap(value)) {
		return value[locale] ?? value.en ?? Object.values(value)[0] ?? '';
	}
	return '';
}
