import type { Theme } from '../../theme';

export type AppLocale = 'zh' | 'en';

/** What the account menu shows and does; the app wires `setTheme`, `setLocale` and sign-out in. */
export interface AccountControls {
	user: { name: string; email: string };
	theme: Theme;
	locale: AppLocale;
	ontheme: (theme: Theme) => void;
	onlocale: (locale: AppLocale) => void;
	/** Sign out, after the app has confirmed any discard of unsaved work. */
	onsignout: () => void;
	/** Where "Personal settings" goes. */
	settingsHref?: string;
}

export const PERSONAL_SETTINGS_HREF = '/settings';

export const THEME_OPTIONS: ReadonlyArray<{ value: Theme; labelKey: string }> = [
	{ value: 'light', labelKey: 'shell.account.themeLight' },
	{ value: 'dark', labelKey: 'shell.account.themeDark' },
	{ value: 'system', labelKey: 'shell.account.themeSystem' }
];

/** Language names are written in their own language and never translated. */
export const LOCALE_OPTIONS: ReadonlyArray<{ value: AppLocale; label: string }> = [
	{ value: 'zh', label: '中文' },
	{ value: 'en', label: 'English' }
];

/** 'zh' for anything but 'en', as the i18n layer does. */
export function normalizeAppLocale(value: string | null | undefined): AppLocale {
	return value === 'en' ? 'en' : 'zh';
}
