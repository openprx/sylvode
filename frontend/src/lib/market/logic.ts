import type { PackageDetail, PreviewRequest, Risk } from './api';

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
