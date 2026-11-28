import { renderMarkdown } from '../utils/markdown';
import type { IndexPluginDetail, IndexPublisherKey, IndexVersion } from './index-api';

export type Role = 'owner' | 'admin' | 'member';

/**
 * CommonMark subset for a plugin description, rendered without raw HTML (the shared renderer
 * escapes it), without images (no remote pixels: the index ships its images through the asset
 * route only) and with https links only. Anything else keeps its text and loses the target.
 */
export function descriptionHtml(source: string): string {
	const withoutImages = source.replace(/!\[([^\]]*)\]\([^)]*\)/g, '$1');
	const httpsLinksOnly = withoutImages.replace(
		/\[([^\]]+)\]\(([^)\s]*)\)/g,
		(_all, text, target) =>
			/^https:\/\/[^\s]+$/i.test(target) ? `[${text}](${target})` : String(text)
	);
	return renderMarkdown(httpsLinksOnly).replace(/<img\b[^>]*>/gi, '');
}

/** The host of a download URL, or an empty string for text that is not a URL. */
export function hostOf(url: string): string {
	try {
		return new URL(url).host;
	} catch {
		return '';
	}
}

/** The first 12 hex digits of a `sha256` with an ellipsis, enough to compare by eye. */
export function shortHash(hash: string): string {
	const clean = hash.replace(/^sha256:/, '');
	return clean.length > 16 ? `${clean.slice(0, 12)}…${clean.slice(-4)}` : clean;
}

/** A key id as a fingerprint in groups of four hex digits (`ed25519:` prefix kept apart). */
export function fingerprint(keyId: string): { algorithm: string; groups: string } {
	const [algorithm = '', hex = ''] = keyId.includes(':') ? keyId.split(':') : ['', keyId];
	return { algorithm, groups: hex.replace(/(.{4})/g, '$1 ').trim() };
}

export function formatSize(bytes: number, locale: string): string {
	const units = ['B', 'KiB', 'MiB'];
	let value = bytes;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit++;
	}
	const text = new Intl.NumberFormat(locale, {
		maximumFractionDigits: unit === 0 ? 0 : 1
	}).format(value);
	return `${text} ${units[unit]}`;
}

/** Semver precedence of two exact versions (release parts, then pre-release is lower). */
export function compareVersions(a: string, b: string): number {
	const split = (v: string) => {
		const [core = '', pre] = v.split('-', 2) as [string, string | undefined];
		return { nums: core.split('.').map((n) => Number.parseInt(n, 10) || 0), pre };
	};
	const left = split(a);
	const right = split(b);
	for (let i = 0; i < 3; i++) {
		const diff = (left.nums[i] ?? 0) - (right.nums[i] ?? 0);
		if (diff !== 0) return diff;
	}
	if (left.pre === right.pre) return 0;
	if (left.pre === undefined) return 1;
	if (right.pre === undefined) return -1;
	return left.pre < right.pre ? -1 : 1;
}

/** An index entry newer than the installed version, when the index offers one for this build. */
export function updateAvailable(installed: string, latestCompatible: string | null): boolean {
	return latestCompatible !== null && compareVersions(latestCompatible, installed) > 0;
}

export interface TrustPlan {
	/** The signing key of the version, as the index lists it. */
	key: IndexPublisherKey | null;
	/** The key is already pinned in this workspace: any admin may import. */
	pinned: boolean;
	/** The workspace must first pin this key (publisher id and fingerprint shown to the owner). */
	needsTrust: boolean;
	/** The publisher already has a pinned key and this is another one. */
	keyChange: boolean;
	/** The pinned keys a key change replaces. */
	pinnedKeyIds: string[];
	/** Who can proceed. */
	canImport: boolean;
	/** Why the person cannot, for the disabled button. */
	blocked: 'member' | 'owner_only' | 'key_revoked' | 'unknown_key' | null;
}

/** What importing `version` needs in this workspace, from the detail the server answered. */
export function trustPlan(detail: IndexPluginDetail, version: IndexVersion, role: Role): TrustPlan {
	const key =
		detail.publisher.keys.find((candidate) => candidate.key_id === version.key_id) ?? null;
	const pinnedKeyIds = detail.publisher.keys
		.filter((candidate) => candidate.pinned && candidate.status === 'active')
		.map((candidate) => candidate.key_id);
	const pinned = key?.pinned === true;
	const needsTrust = key !== null && !pinned && key.status === 'active';
	const keyChange = needsTrust && pinnedKeyIds.length > 0;
	let blocked: TrustPlan['blocked'] = null;
	if (role === 'member') blocked = 'member';
	else if (key === null) blocked = 'unknown_key';
	else if (key.status !== 'active') blocked = 'key_revoked';
	else if (needsTrust && role !== 'owner') blocked = 'owner_only';
	return { key, pinned, needsTrust, keyChange, pinnedKeyIds, canImport: blocked === null, blocked };
}

/** Facet values (category, label ids) seen in results, kept sorted. */
export function mergeFacet(known: readonly string[], seen: readonly string[]): string[] {
	const next = new Set(known);
	for (const value of seen) if (value) next.add(value);
	return [...next].sort((a, b) => a.localeCompare(b));
}

/** The stable codes of a refused import that have their own wording under `market.index.error`. */
export const IMPORT_ERROR_CODES = [
	'publisher_untrusted',
	'publisher_trust_owner_only',
	'index_version_not_importable',
	'index_download_failed',
	'index_size_mismatch',
	'index_sha256_mismatch',
	'index_mismatch',
	'index_key_conflict',
	'index_key_revoked',
	'index_import_in_progress',
	'index_source_not_found',
	'index_plugin_not_found',
	'import_request_invalid'
] as const;

/** `details.reason` or `details.field` of a refusal, if it is a string. */
export function detailReason(details: unknown): string | null {
	if (typeof details !== 'object' || details === null) return null;
	const record = details as Record<string, unknown>;
	for (const key of ['reason', 'field']) {
		const value = record[key];
		if (typeof value === 'string') return value;
	}
	return null;
}

/** Seconds a refusal asks to wait (`retry_after_secs`), if any. */
export function retryAfter(details: unknown): number | null {
	if (typeof details !== 'object' || details === null) return null;
	const value = (details as Record<string, unknown>).retry_after_secs;
	return typeof value === 'number' ? value : null;
}

/** The newest listed, compatible version of a plugin detail (what the page offers first). */
export function defaultVersion(detail: IndexPluginDetail): IndexVersion | null {
	return (
		detail.versions.find((version) => version.version === detail.latest_compatible) ??
		detail.versions.find((version) => version.status === 'listed') ??
		detail.versions[0] ??
		null
	);
}

/** Unix seconds (index) or an ISO string (API) as a short local date. */
export function shortDate(
	value: number | string | null | undefined,
	locale: string,
	withTime = true
): string {
	if (value === null || value === undefined || value === '') return '';
	const date = typeof value === 'number' ? new Date(value * 1000) : new Date(value);
	if (Number.isNaN(date.getTime())) return '';
	return new Intl.DateTimeFormat(
		locale,
		withTime ? { dateStyle: 'medium', timeStyle: 'short' } : { dateStyle: 'medium' }
	).format(date);
}
