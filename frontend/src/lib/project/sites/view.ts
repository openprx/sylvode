/** Pure helpers of the sites console: titles, slugs, origins, the publish summary and error codes. */
import type { SiteBinding, SitePage, SiteSettings, SiteStatus, SiteText } from './wire';

export type Tone = 'neutral' | 'success' | 'warning' | 'danger';

export const MAX_TEXT_CHARS = 200;
export const MAX_FRAME_ANCESTORS = 8;

export function statusTone(status: SiteStatus): Tone {
	switch (status) {
		case 'published':
			return 'success';
		case 'disabled':
			return 'danger';
		default:
			return 'neutral';
	}
}

/** The title in the current locale, else `en`, else any value the map holds. */
export function siteTitle(title: SiteText, locale: string | null | undefined): string {
	if (typeof title === 'string') return title;
	const lang = (locale ?? '').split('-')[0] ?? '';
	return title[lang] ?? title.en ?? Object.values(title)[0] ?? '';
}

/** The title value to send: a plain string, or a map when a Chinese title is given too. */
export function titleValue(en: string, zh: string): SiteText {
	const english = en.trim();
	const chinese = zh.trim();
	return chinese ? { en: english, zh: chinese } : english;
}

/** The form fields (`en`, `zh`) of a stored title. */
export function titleFields(title: SiteText): { en: string; zh: string } {
	if (typeof title === 'string') return { en: title, zh: '' };
	return { en: title.en ?? '', zh: title.zh ?? '' };
}

export function validText(text: string): boolean {
	const trimmed = text.trim();
	return trimmed.length > 0 && [...trimmed].length <= MAX_TEXT_CHARS;
}

/** Two to forty lowercase letters, digits or hyphens, not starting with a hyphen. */
export function isSlug(slug: string): boolean {
	return /^[a-z0-9][a-z0-9-]{1,39}$/.test(slug);
}

/** A slug proposed from a title; empty when the title has no usable letters or digits. */
export function slugFromTitle(title: string): string {
	const slug = title
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, '-')
		.replace(/^-+/, '')
		.slice(0, 40)
		.replace(/-+$/, '');
	return slug.length >= 2 ? slug : '';
}

/** One exact https origin: scheme and host (and port), no path, query, fragment or credentials. */
export function isHttpsOrigin(value: string): boolean {
	let url: URL;
	try {
		url = new URL(value);
	} catch {
		return false;
	}
	return (
		url.protocol === 'https:' &&
		url.hostname !== '' &&
		!url.hostname.includes('*') &&
		url.username === '' &&
		url.password === '' &&
		url.search === '' &&
		url.hash === '' &&
		url.origin === value
	);
}

/** Lines of a textarea as a list: trimmed, empty lines dropped, duplicates removed. */
export function originLines(text: string): string[] {
	return [
		...new Set(
			text
				.split(/\r?\n/)
				.map((line) => line.trim())
				.filter((line) => line !== '')
		)
	];
}

export type SettingsProblem = 'allowed_origin_invalid' | 'frame_ancestors_invalid' | null;

/** The first reason these settings would be refused (the server decides again). */
export function settingsProblem(allowedOrigin: string, ancestors: string[]): SettingsProblem {
	if (allowedOrigin.trim() !== '' && !isHttpsOrigin(allowedOrigin.trim()))
		return 'allowed_origin_invalid';
	if (ancestors.length > MAX_FRAME_ANCESTORS || !ancestors.every(isHttpsOrigin))
		return 'frame_ancestors_invalid';
	return null;
}

/**
 * The body of `PUT /settings`. The call replaces every key, so the stored `theme` is sent back
 * unchanged; an empty origin or list is left out (that clears it).
 */
export function settingsBody(
	stored: SiteSettings,
	allowedOrigin: string,
	ancestors: string[]
): SiteSettings {
	const body: SiteSettings = {};
	const origin = allowedOrigin.trim();
	if (origin !== '') body.allowed_origin = origin;
	if (ancestors.length > 0) body.frame_ancestors = ancestors;
	if (stored.theme) body.theme = stored.theme;
	return body;
}

/** The binding key a layout data source names (`$bind.<key>` or `form:<key>`). */
export function bindingOf(form: string): string | null {
	if (form.startsWith('$bind.')) return form.slice('$bind.'.length);
	if (form.startsWith('form:')) return form.slice('form:'.length);
	return null;
}

/** The binding keys a layout spec uses, from its `data` sources. */
export function layoutBindingKeys(spec: unknown): string[] {
	const data = (spec as { data?: unknown } | null)?.data;
	if (!data || typeof data !== 'object') return [];
	const keys = new Set<string>();
	for (const source of Object.values(data as Record<string, unknown>)) {
		const form = (source as { form?: unknown } | null)?.form;
		const key = typeof form === 'string' ? bindingOf(form) : null;
		if (key) keys.add(key);
	}
	return [...keys];
}

export type PublishWarning =
	| { code: 'no_visitor_page' }
	| { code: 'no_home_page' }
	| { code: 'missing_binding'; route: string; key: string }
	| { code: 'no_url' };

export interface PublishInput {
	pages: SitePage[];
	bindings: SiteBinding[];
	/** Binding keys named by the layout of each page, by page id. */
	pageBindingKeys: Record<string, string[]>;
	url: string | null;
}

export interface PublishSummary {
	visitorPages: number;
	/** Bindings a visitor can submit to or read through. */
	publicBindings: { key: string; submit: number; read: number }[];
	warnings: PublishWarning[];
}

/** What becomes public and what looks wrong; none of it blocks publishing. */
export function publishSummary(input: PublishInput): PublishSummary {
	const visitor = input.pages.filter((page) => page.audience === 'visitor');
	const known = new Set(input.bindings.map((binding) => binding.binding_key));
	const warnings: PublishWarning[] = [];
	if (visitor.length === 0) warnings.push({ code: 'no_visitor_page' });
	if (!visitor.some((page) => page.route === '/')) warnings.push({ code: 'no_home_page' });
	for (const page of input.pages) {
		for (const key of input.pageBindingKeys[page.id] ?? []) {
			if (!known.has(key)) warnings.push({ code: 'missing_binding', route: page.route, key });
		}
	}
	if (!input.url) warnings.push({ code: 'no_url' });
	const publicBindings = input.bindings
		.map((binding) => {
			const exposed = binding.projection.visitor;
			const read = exposed?.read && exposed.read.scope !== 'none' ? exposed.read.fields.length : 0;
			return { key: binding.binding_key, submit: exposed?.submit?.fields.length ?? 0, read };
		})
		.filter((binding) => binding.submit > 0 || binding.read > 0);
	return { visitorPages: visitor.length, publicBindings, warnings };
}

/** Error codes the console words itself (`sites.errors.<code>`); any other falls back to the server text. */
export const KNOWN_ERRORS: ReadonlySet<string> = new Set([
	'slug_invalid',
	'slug_taken',
	'title_invalid',
	'site_not_draft',
	'site_published',
	'allowed_origin_invalid',
	'frame_ancestors_invalid',
	'update_empty',
	'user_not_member'
]);

export function knownError(code: string | undefined): string | null {
	return code && KNOWN_ERRORS.has(code) ? code : null;
}

/** Tabs of the site page. Only Settings exists in this batch; the others join with their screens. */
export const SITE_TABS = ['settings'] as const;
export type SiteTab = (typeof SITE_TABS)[number];

export function tabOf(raw: string | null): SiteTab {
	return (SITE_TABS as readonly string[]).includes(raw ?? '') ? (raw as SiteTab) : SITE_TABS[0];
}
