/** Pure helpers for the "Connected applications" rows and the consent scope list. */

/** The three groups of the scope list (contract 14.1 item 6). */
export type ScopeGroup = 'read' | 'change' | 'other';

export function scopeGroup(name: string): ScopeGroup {
	if (name.endsWith(':read')) return 'read';
	if (name.endsWith(':write')) return 'change';
	return 'other';
}

export const SCOPE_GROUP_ORDER: readonly ScopeGroup[] = ['read', 'change', 'other'];

/** Scope names the page has a translated label for (`oauth.scope.<name>`). */
const KNOWN_SCOPES: ReadonlySet<string> = new Set([
	'projects:read',
	'work_items:read',
	'work_items:write',
	'forms:read',
	'forms:write',
	'notifications:read',
	'notifications:write',
	'tasks:read',
	'offline_access'
]);

export function isKnownScope(name: string): boolean {
	return KNOWN_SCOPES.has(name);
}

/** The chip on a connection: "Read" or "Read and change". */
export function scopeSummary(scopes: readonly string[]): 'read' | 'readWrite' {
	return scopes.some((scope) => scopeGroup(scope) === 'change') ? 'readWrite' : 'read';
}

/** Scopes in vocabulary groups, empty groups left out. */
export function groupScopes<T extends { name: string }>(
	scopes: readonly T[]
): Array<{ group: ScopeGroup; scopes: T[] }> {
	return SCOPE_GROUP_ORDER.map((group) => ({
		group,
		scopes: scopes.filter((scope) => scopeGroup(scope.name) === group)
	})).filter((entry) => entry.scopes.length > 0);
}

const UNITS: ReadonlyArray<[Intl.RelativeTimeFormatUnit, number]> = [
	['year', 365 * 24 * 3600],
	['month', 30 * 24 * 3600],
	['day', 24 * 3600],
	['hour', 3600],
	['minute', 60]
];

/** "10 minutes ago" in `lang`; "just now" below a minute. Null for an unreadable timestamp. */
export function relativeTime(iso: string, now: Date, lang: string, justNow: string): string | null {
	const then = Date.parse(iso);
	if (Number.isNaN(then)) return null;
	const seconds = Math.round((then - now.getTime()) / 1000);
	if (Math.abs(seconds) < 60) return justNow;
	const format = new Intl.RelativeTimeFormat(lang, { numeric: 'always' });
	for (const [unit, size] of UNITS) {
		if (Math.abs(seconds) >= size) {
			const text = format.format(Math.round(seconds / size), unit);
			// Chinese writes a space between the number and the unit ("10 分钟前").
			return lang.startsWith('zh') ? text.replace(/(\d)(?=[\u4e00-\u9fff])/g, '$1 ') : text;
		}
	}
	return justNow;
}

/** A date for the expanded details, in the person's language. */
export function formatDate(iso: string, lang: string): string {
	const time = Date.parse(iso);
	if (Number.isNaN(time)) return iso;
	return new Intl.DateTimeFormat(lang, { dateStyle: 'medium', timeStyle: 'short' }).format(
		new Date(time)
	);
}

/** "Ramflux · MacBook", or just the application name without a device name. */
export function connectionLabel(displayName: string, deviceName: string | null): string {
	return deviceName ? `${displayName} · ${deviceName}` : displayName;
}

export type Translate = (key: string, options?: { values?: Record<string, unknown> }) => string;

/** The plain-language label of a scope; a name the page does not know is shown as named. */
export function scopeText(translate: Translate, name: string, client: string): string {
	return isKnownScope(name)
		? translate(`oauth.scope.${name}`, { values: { client } })
		: translate('oauth.scope.unknown', { values: { name } });
}
