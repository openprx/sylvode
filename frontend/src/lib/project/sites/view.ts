/** Pure helpers of the sites console: titles, slugs, origins, the publish summary and error codes. */
import type { SiteBinding, SitePage, SiteRole, SiteSettings, SiteStatus, SiteText } from './wire';

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
	'user_not_member',
	'role_invalid',
	'route_invalid',
	'route_taken',
	'audience_invalid',
	'nav_invalid',
	'layout_required',
	'layout_invalid',
	'package_page_readonly',
	'site_route_taken'
]);

export function knownError(code: string | undefined): string | null {
	return code && KNOWN_ERRORS.has(code) ? code : null;
}

/** Tabs of the site page. Form bindings join with their screen. */
export const SITE_TABS = ['pages', 'roles', 'settings', 'drafts'] as const;
export type SiteTab = (typeof SITE_TABS)[number];

export function tabOf(raw: string | null): SiteTab {
	return (SITE_TABS as readonly string[]).includes(raw ?? '') ? (raw as SiteTab) : SITE_TABS[0];
}

/** Whether the viewer changes pages: owners and admins, and the site's own editors. */
export function canEditPages(isAdmin: boolean, role: SiteRole['role'] | null): boolean {
	return isAdmin || role === 'site_editor';
}

/**
 * A route as the server accepts it: `/`, or lowercase segments of letters, digits and hyphens;
 * a segment `:name` (lowercase letters and `_`) takes a record id and cannot be the first.
 */
export function isRoute(route: string): boolean {
	if (route === '/') return true;
	if (!route.startsWith('/') || [...route].length > 200) return false;
	return route
		.slice(1)
		.split('/')
		.every((part, index) =>
			part.startsWith(':')
				? index > 0 && /^[a-z_]+$/.test(part.slice(1))
				: /^[a-z0-9-]+$/.test(part)
		);
}

export const isPackagePage = (page: Pick<SitePage, 'installation_id'>): boolean =>
	page.installation_id !== null;

/** The text of the menu label to send: `null` (not in the menu) when both fields are empty. */
export function navValue(en: string, zh: string): SiteText | null {
	return en.trim() === '' && zh.trim() === '' ? null : titleValue(en, zh);
}

/** The form fields of a stored menu label. */
export function navFields(nav: SiteText | null): { en: string; zh: string } {
	return nav === null ? { en: '', zh: '' } : titleFields(nav);
}

/** A menu label needs an English text when any is given (the server requires `en`). */
export function navProblem(en: string, zh: string): boolean {
	if (en.trim() === '' && zh.trim() === '') return false;
	return !validText(en) || (zh.trim() !== '' && !validText(zh));
}

export type PageField = 'route' | 'audience' | 'nav' | 'layout';

/** The page dialog field an error code belongs under; `null` for a code that fits no field. */
export function pageErrorField(code: string | undefined): PageField | null {
	switch (code) {
		case 'route_invalid':
		case 'route_taken':
			return 'route';
		case 'audience_invalid':
			return 'audience';
		case 'nav_invalid':
			return 'nav';
		case 'layout_required':
		case 'layout_invalid':
			return 'layout';
		default:
			return null;
	}
}

/** The paths a `layout_invalid` refusal lists (`details.errors[].path`), for the line under the select. */
export function layoutIssues(details: unknown): string[] {
	const errors = (details as { errors?: unknown } | null | undefined)?.errors;
	if (!Array.isArray(errors)) return [];
	return errors.flatMap((entry: unknown) => {
		const issue = entry as { path?: unknown; code?: unknown } | null;
		if (!issue || typeof issue.code !== 'string') return [];
		const path = typeof issue.path === 'string' && issue.path !== '' ? issue.path : '/';
		return [`${path}  ${issue.code}`];
	});
}

/**
 * The position changes that move one of the site's own pages one step up (-1) or down (1) among
 * the own pages. Own pages are renumbered from the lowest position they hold, so equal positions
 * (every new page starts at 0) still order. A package page cannot be changed and is left out.
 */
export function moveUpdates(
	pages: SitePage[],
	id: string,
	step: -1 | 1
): { id: string; position: number }[] {
	const own = pages.filter((page) => !isPackagePage(page));
	const at = own.findIndex((page) => page.id === id);
	const to = at + step;
	if (at < 0 || to < 0 || to >= own.length) return [];
	const order = [...own];
	const moved = order[at];
	const other = order[to];
	if (!moved || !other) return [];
	order[at] = other;
	order[to] = moved;
	const base = Math.min(...own.map((page) => page.position));
	return order.flatMap((page, index) =>
		page.position === base + index ? [] : [{ id: page.id, position: base + index }]
	);
}

/** Pending `site_page` drafts per site, from one project-wide list. */
export function pendingBySite(drafts: { target: Record<string, unknown> }[]): Map<string, number> {
	const counts = new Map<string, number>();
	for (const draft of drafts) {
		const site = draft.target.site_id;
		if (typeof site === 'string') counts.set(site, (counts.get(site) ?? 0) + 1);
	}
	return counts;
}

/** Whether a draft targets the site. */
export function draftOfSite(draft: { target: Record<string, unknown> }, siteId: string): boolean {
	return draft.target.site_id === siteId;
}

export interface MemberLike {
	user_id: string;
	name: string;
	email: string;
	entity_type?: string;
}

/** Active people who hold no role on the site yet; agents cannot sign in on a site. */
export function roleCandidates(members: MemberLike[], roles: SiteRole[]): MemberLike[] {
	const held = new Set(roles.map((role) => role.user_id));
	return members.filter((member) => member.entity_type !== 'bot' && !held.has(member.user_id));
}
