import type { CatalogueCapability, PackageDetail, PreviewRequest, Risk } from './api';

export const KINDS = ['form-template', 'page-template', 'app', 'plugin'] as const;
export type Kind = (typeof KINDS)[number];

/** The badge tone of a risk: reading is quiet, everything that changes or exposes data is loud. */
export function riskTone(risk: Risk): 'neutral' | 'success' | 'warning' | 'danger' {
	switch (risk) {
		case 'read':
			return 'success';
		case 'write':
			return 'warning';
		case 'destructive':
		case 'admin':
		case 'secret':
		case 'unknown':
			return 'danger';
	}
}

/** The text of a localised map in the viewer's language, else English, else any entry. */
export function pickText(map: Record<string, string> | null | undefined, locale: string): string {
	if (!map) return '';
	const base = locale.split('-')[0] ?? locale;
	return map[locale] ?? map[base] ?? map.en ?? Object.values(map)[0] ?? '';
}

export interface DataAccess {
	/** The package bind name, or the id of the existing form, or null for the whole scope. */
	target: string | null;
	reads: string[];
	writes: string[];
}

/**
 * What the requests let the package read and write, per form they name. A request without a form
 * applies to every form of the scope. Reading is `read`; every other risk changes data.
 */
export function dataAccess(
	requests: readonly PreviewRequest[],
	label: (capability: string) => string
): DataAccess[] {
	const byTarget = new Map<string | null, DataAccess>();
	for (const request of requests) {
		const target = request.selector.form ?? request.form_bind ?? null;
		let entry = byTarget.get(target);
		if (!entry) {
			entry = { target, reads: [], writes: [] };
			byTarget.set(target, entry);
		}
		(request.risk === 'read' ? entry.reads : entry.writes).push(label(request.capability));
	}
	return [...byTarget.values()];
}

/** Pages a visitor can open without an account. */
export function publicPages(detail: Pick<PackageDetail, 'analysis'>): string[] {
	return detail.analysis.pages
		.filter((page) => page.audience === 'visitor')
		.map((page) => page.route);
}

/** The three tiers a review groups requests into; admin, secret and unknown ids count as destructive. */
export type RiskTier = 'read' | 'write' | 'destructive';
export const RISK_TIERS: readonly RiskTier[] = ['read', 'write', 'destructive'];

export function riskTier(risk: Risk): RiskTier {
	if (risk === 'read') return 'read';
	if (risk === 'write') return 'write';
	return 'destructive';
}

/** The area of a capability id: its first segment. */
export function areaOf(capability: string): string {
	return capability.split('.')[0] ?? capability;
}

export interface AreaGroup {
	area: string;
	/** Operations the requested ids open in this area; `null` when the catalogue is not loaded. */
	ops: number | null;
	tiers: { tier: RiskTier; items: { index: number; request: PreviewRequest }[] }[];
}

/** The number of operations behind each capability id, from the plugin catalogue. */
export function opCounts(capabilities: readonly CatalogueCapability[]): Map<string, number> {
	return new Map(capabilities.map((capability) => [capability.id, capability.operations.length]));
}

/**
 * Requests grouped by area (alphabetical), then by risk tier (read, write, destructive), each item
 * keeping its index in the request list so a tick stays attached to it.
 */
export function groupRequests(
	requests: readonly PreviewRequest[],
	counts: ReadonlyMap<string, number> | null
): AreaGroup[] {
	const byArea = new Map<string, AreaGroup>();
	requests.forEach((request, index) => {
		const area = areaOf(request.capability);
		let group = byArea.get(area);
		if (!group) {
			group = { area, ops: counts ? 0 : null, tiers: [] };
			byArea.set(area, group);
		}
		if (group.ops !== null) group.ops += counts?.get(request.capability) ?? 0;
		const tier = riskTier(request.risk);
		let entry = group.tiers.find((candidate) => candidate.tier === tier);
		if (!entry) {
			entry = { tier, items: [] };
			group.tiers.push(entry);
		}
		entry.items.push({ index, request });
	});
	for (const group of byArea.values()) {
		group.tiers.sort((a, b) => RISK_TIERS.indexOf(a.tier) - RISK_TIERS.indexOf(b.tier));
	}
	return [...byArea.values()].sort((a, b) => a.area.localeCompare(b.area));
}

/** Catalogue capabilities that match a search text on id, operation key, path or comment. */
export function searchCatalogue(
	capabilities: readonly CatalogueCapability[],
	text: string
): CatalogueCapability[] {
	const needle = text.trim().toLowerCase();
	if (needle === '') return [...capabilities];
	const out: CatalogueCapability[] = [];
	for (const capability of capabilities) {
		if (capability.id.toLowerCase().includes(needle)) {
			out.push(capability);
			continue;
		}
		const operations = capability.operations.filter((operation) =>
			`${operation.op} ${operation.method} ${operation.path} ${operation.comment ?? ''}`
				.toLowerCase()
				.includes(needle)
		);
		if (operations.length > 0) out.push({ ...capability, operations });
	}
	return out;
}
