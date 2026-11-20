import { browser } from '$app/env';
import { writable, type Readable } from 'svelte/store';

export type Theme = 'light' | 'dark' | 'system';

const THEME_KEY = 'theme';
const DEFAULT_THEME: Theme = 'light';

function getStoredTheme(): Theme {
	if (!browser) return DEFAULT_THEME;
	const stored = localStorage.getItem(THEME_KEY);
	if (stored === 'light' || stored === 'dark' || stored === 'system') {
		return stored;
	}
	return DEFAULT_THEME;
}

export const theme = writable<Theme>(getStoredTheme());

export type ResolvedTheme = 'light' | 'dark';

/** 'system' resolved against the OS preference: what is actually shown. */
export function resolveTheme(value: Theme, prefersDark: boolean): ResolvedTheme {
	return value === 'dark' || (value === 'system' && prefersDark) ? 'dark' : 'light';
}

function systemPrefersDark(): boolean {
	return browser && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

const resolvedStore = writable<ResolvedTheme>(resolveTheme(getStoredTheme(), systemPrefersDark()));
/** The theme in effect (never 'system'); follows `setTheme` and OS changes while on 'system'. */
export const resolvedTheme: Readable<ResolvedTheme> = { subscribe: resolvedStore.subscribe };

export function applyTheme(nextTheme: Theme) {
	if (!browser) return;
	const root = document.documentElement;
	const prefersDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
	const resolved = resolveTheme(nextTheme, prefersDark);
	root.classList.toggle('dark', resolved === 'dark');
	resolvedStore.set(resolved);
}

export function setTheme(nextTheme: Theme) {
	theme.set(nextTheme);
	if (!browser) return;
	localStorage.setItem(THEME_KEY, nextTheme);
	applyTheme(nextTheme);
}

if (browser) {
	const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)');
	mediaQuery.addEventListener('change', () => {
		if (getStoredTheme() === 'system') {
			applyTheme('system');
		}
	});
}
