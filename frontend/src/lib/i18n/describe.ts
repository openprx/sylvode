import { get } from 'svelte/store';
// The relative path is the stand-in the app aliases `svelte-i18n` to (`vite.config.ts`): the same
// store in the app, and importable by the unit tests, where the package itself is not resolvable.
import { t } from './svelte-i18n';

type Translate = (key: string, options?: { values?: Record<string, string | number> }) => string;

interface Describable {
	message?: string;
	messageKey?: string;
	messageValues?: Record<string, string | number>;
}

/**
 * The words for a failed result. A failure the client produced itself carries a `messageKey`
 * (`transport.*`) and no text, so the modules that make it never need an initialised locale;
 * the toast or state block that shows it calls this. Otherwise the server's message is used,
 * then `fallback`. `translate` defaults to the app locale and is replaced in tests.
 */
export function describeResult(
	result: Describable,
	fallback = '',
	translate: Translate = (key, options) => get(t)(key, options)
): string {
	if (result.messageKey) return translate(result.messageKey, { values: result.messageValues });
	return result.message || fallback;
}
