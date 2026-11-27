import { browser } from '$app/env';
import { init, locale, register } from 'svelte-i18n';
import { mergeMessages, type Messages } from '../ui/foundations/messages';

// Each `lib/ui` component and each feature folder under `lib/` (`shell`, `surface`, `project`, `oauth`,
// `milestones`, `notifications`, `governance`, `admin`, `setup`, `flow`, `webhooks`, `people`, `nav`, `forms`, `market`) keeps its own
// messages in `locale.<lang>.json` next to the code; they are merged into the app dictionary here, so
// adding a feature never edits the shared locale files. A new feature folder is added to both globs and to
// the list in `tests/features/locale-modules.test.ts`.
const uiZh = import.meta.glob<Messages>(
	'../{ui,shell,surface,project,oauth,milestones,notifications,governance,admin,setup,flow,webhooks,people,nav,forms,market}/**/locale.zh.json',
	{
		eager: true,
		import: 'default'
	}
);
const uiEn = import.meta.glob<Messages>(
	'../{ui,shell,surface,project,oauth,milestones,notifications,governance,admin,setup,flow,webhooks,people,nav,forms,market}/**/locale.en.json',
	{
		eager: true,
		import: 'default'
	}
);

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
