import { browser } from '$app/env';
import { init, locale, register } from 'svelte-i18n';
import { mergeMessages, type Messages } from '../ui/foundations/messages';

// Each `lib/ui` component (and each `lib/shell`, `lib/surface` and `lib/project` folder) keeps its own
// messages in `locale.<lang>.json` next to the code (`ui.*`, `states.*`, `shell.*`, `surface.*`); they are merged into the app dictionary here, so adding a component never
// edits the shared locale files.
const uiZh = import.meta.glob<Messages>('../{ui,shell,surface,project}/**/locale.zh.json', {
	eager: true,
	import: 'default'
});
const uiEn = import.meta.glob<Messages>('../{ui,shell,surface,project}/**/locale.en.json', {
	eager: true,
	import: 'default'
});

register('zh', async () =>
	mergeMessages((await import('./zh.json')).default as Messages, ...Object.values(uiZh))
);
register('en', async () =>
	mergeMessages((await import('./en.json')).default as Messages, ...Object.values(uiEn))
);

const supportedLocales = new Set(['zh', 'en']);

function normalizeLocale(lang: string | null): 'zh' | 'en' {
	return lang === 'en' ? 'en' : 'zh';
}

function syncDocumentLang(lang: string): void {
	if (browser) {
		document.documentElement.lang = normalizeLocale(lang);
	}
}

const savedLocale =
	browser && supportedLocales.has(localStorage.getItem('locale') ?? '')
		? normalizeLocale(localStorage.getItem('locale'))
		: 'zh';

init({
	fallbackLocale: 'zh',
	initialLocale: savedLocale
});

export function setLocale(lang: string): void {
	const next = normalizeLocale(lang);
	locale.set(next);
	syncDocumentLang(next);
	if (browser) {
		localStorage.setItem('locale', next);
	}
}

syncDocumentLang(savedLocale);
