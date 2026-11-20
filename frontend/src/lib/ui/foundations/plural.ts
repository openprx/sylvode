/**
 * Plural wording. The app's i18n (`svelte-i18n` with plain JSON messages) has no ICU plural
 * syntax, so a message that depends on a count is stored as one key per plural category:
 *
 *   "ui.dataTable.selected": { "one": "1 row selected", "other": "{count} rows selected" }
 *
 * and read with `plural($t, 'ui.dataTable.selected', count, $locale)`.
 *
 * - The category comes from `Intl.PluralRules` for the app language (`en`: `one` / `other`).
 * - Chinese has one form, so `zh` files define only `other`; the locale parity check ignores the
 *   categories a language does not use.
 * - A missing category falls back to `other`; an unknown or missing language falls back to `en`.
 * - The count is passed to the message as `{count}`; add more `values` when the sentence needs them.
 */

export type PluralCategory = 'zero' | 'one' | 'two' | 'few' | 'many' | 'other';

/** The categories a plural key may use; `other` is the required fallback. */
export const PLURAL_CATEGORIES: readonly PluralCategory[] = [
	'zero',
	'one',
	'two',
	'few',
	'many',
	'other'
];

/** The plural category of `count` in `lang` (BCP 47 tag, as `zh` or `en`). */
export function pluralCategory(count: number, lang?: string | null): PluralCategory {
	for (const tag of [lang, 'en']) {
		if (!tag) continue;
		try {
			return new Intl.PluralRules(tag).select(count) as PluralCategory;
		} catch {
			// Unsupported or malformed tag: try the next one.
		}
	}
	return 'other';
}

/** The message keys to try for `count`, best first: the category key, then `other`. */
export function pluralKeys(base: string, count: number, lang?: string | null): string[] {
	const category = pluralCategory(count, lang);
	return category === 'other' ? [`${base}.other`] : [`${base}.${category}`, `${base}.other`];
}

type Translate = (
	key: string,
	options?: { values?: Record<string, unknown>; default?: string }
) => string;

/** The message for `count`: `plural($t, 'ui.dataTable.selected', 3, $locale)`. */
export function plural(
	translate: Translate,
	base: string,
	count: number,
	lang?: string | null,
	values: Record<string, unknown> = {}
): string {
	const all = { ...values, count };
	const [first, ...rest] = pluralKeys(base, count, lang);
	const fallback = rest.length ? translate(rest[0], { values: all }) : undefined;
	return translate(
		first,
		fallback === undefined ? { values: all } : { values: all, default: fallback }
	);
}

/** True for a key segment that names a plural category (`...selected.one`). */
export function isPluralCategoryKey(key: string): boolean {
	const last = key.slice(key.lastIndexOf('.') + 1);
	return last !== 'other' && (PLURAL_CATEGORIES as readonly string[]).includes(last);
}
