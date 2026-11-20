import { fromStore } from 'svelte/store';
import { locale, t } from 'svelte-i18n';

// `fromStore(...).current` subscribes when read inside an effect or a derived and falls back to the
// store's current value elsewhere, which is what `$t` does in a component.
const tState = fromStore(t);
const localeState = fromStore(locale);

export const translate: typeof tState.current = (...args) => tState.current(...args);

export function currentLocale(): string | null | undefined {
	return localeState.current;
}
