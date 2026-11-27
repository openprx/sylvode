/**
 * Editing rules of a navigation document, pure: every function returns a new document. The
 * server validates again on save; these keep the editor from building what it would refuse.
 */
import { PROJECT_BUILTINS, WORKSPACE_BUILTINS } from '../builtins';
import type {
	LocaleLabels,
	NavDoc,
	NavEntryDoc,
	NavScope,
	NavSectionDoc,
	NavTarget
} from '../types';

export const MAX_SECTIONS = 20;
export const MAX_ENTRIES = 200;
export const MAX_LABEL = 64;

/** Glyphs an entry may use (the server's list). */
export const ICONS = [
	'house',
	'bell',
	'list-checks',
	'clock',
	'folder',
	'folder-kanban',
	'layout-list',
	'kanban',
	'repeat',
	'table-2',
	'file-text',
	'app-window',
	'settings',
	'workflow',
	'bot',
	'sparkles',
	'landmark',
	'flag',
	'users',
	'briefcase',
	'link',
	'external-link',
	'star',
	'layout-template',
	'globe',
	'puzzle'
] as const;

export interface EntryPosition {
	section: number;
	index: number;
}

const ID = /^[a-z0-9][a-z0-9_-]{0,63}$/;

export function validId(id: string): boolean {
	return ID.test(id);
}

/** Every section and entry id of a document (they share one namespace). */
export function usedIds(doc: NavDoc): Set<string> {
	const ids = new Set<string>();
	for (const section of doc.sections) {
		ids.add(section.id);
		for (const entry of section.entries) ids.add(entry.id);
	}
	return ids;
}

/** A free id derived from `base` (a label or a kind). */
export function uniqueId(doc: NavDoc, base: string): string {
	const slug =
		base
			.toLowerCase()
			.replace(/[^a-z0-9_-]+/g, '-')
			.replace(/^[^a-z0-9]+/, '')
			.replace(/-+$/, '')
			.slice(0, 40) || 'item';
	const used = usedIds(doc);
	if (!used.has(slug)) return slug;
	for (let n = 2; ; n++) {
		const next = `${slug}-${n}`;
		if (!used.has(next)) return next;
	}
}

export function findEntry(
	doc: NavDoc,
	id: string
): (EntryPosition & { entry: NavEntryDoc }) | null {
	for (const [section, s] of doc.sections.entries()) {
		const index = s.entries.findIndex((e) => e.id === id);
		if (index >= 0) return { section, index, entry: s.entries[index] };
	}
	return null;
}

export function entryCount(doc: NavDoc): number {
	return doc.sections.reduce((n, s) => n + s.entries.length, 0);
}

function withSections(doc: NavDoc, sections: NavSectionDoc[]): NavDoc {
	return { ...doc, sections };
}

/** Moves an entry to `to` (an index counted after the entry has been taken out). */
export function moveEntry(doc: NavDoc, id: string, to: EntryPosition): NavDoc {
	const from = findEntry(doc, id);
	if (!from || !doc.sections[to.section]) return doc;
	const sections = doc.sections.map((s) => ({ ...s, entries: [...s.entries] }));
	const [entry] = sections[from.section].entries.splice(from.index, 1);
	const target = sections[to.section].entries;
	target.splice(Math.max(0, Math.min(to.index, target.length)), 0, entry);
	return withSections(doc, sections);
}

/** One step up or down; at the edge of a section the entry crosses into the neighbouring one. */
export function stepEntry(doc: NavDoc, id: string, direction: -1 | 1): NavDoc {
	const at = findEntry(doc, id);
	if (!at) return doc;
	const size = doc.sections[at.section].entries.length;
	const next = at.index + direction;
	if (next >= 0 && next < size) return moveEntry(doc, id, { section: at.section, index: next });
	const neighbour = at.section + direction;
	if (!doc.sections[neighbour]) return doc;
	return moveEntry(doc, id, {
		section: neighbour,
		index: direction < 0 ? doc.sections[neighbour].entries.length : 0
	});
}

export function stepSection(doc: NavDoc, sectionId: string, direction: -1 | 1): NavDoc {
	const from = doc.sections.findIndex((s) => s.id === sectionId);
	const to = from + direction;
	if (from < 0 || to < 0 || to >= doc.sections.length) return doc;
	const sections = [...doc.sections];
	[sections[from], sections[to]] = [sections[to], sections[from]];
	return withSections(doc, sections);
}

export function addSection(doc: NavDoc, label: LocaleLabels | null): NavDoc | null {
	if (doc.sections.length >= MAX_SECTIONS) return null;
	const id = uniqueId(doc, label?.en ?? label?.zh ?? 'section');
	return withSections(doc, [...doc.sections, { id, label, collapsed: false, entries: [] }]);
}

export function updateSection(
	doc: NavDoc,
	id: string,
	patch: Partial<Pick<NavSectionDoc, 'label' | 'collapsed'>>
): NavDoc {
	return withSections(
		doc,
		doc.sections.map((s) => (s.id === id ? { ...s, ...patch } : s))
	);
}

/** Only an empty section can be removed. */
export function removeSection(doc: NavDoc, id: string): NavDoc {
	return withSections(
		doc,
		doc.sections.filter((s) => s.id !== id || s.entries.length > 0)
	);
}

export function addEntry(doc: NavDoc, sectionId: string, entry: NavEntryDoc): NavDoc | null {
	if (entryCount(doc) >= MAX_ENTRIES || usedIds(doc).has(entry.id)) return null;
	return withSections(
		doc,
		doc.sections.map((s) => (s.id === sectionId ? { ...s, entries: [...s.entries, entry] } : s))
	);
}

export function updateEntry(doc: NavDoc, id: string, next: NavEntryDoc): NavDoc {
	return withSections(
		doc,
		doc.sections.map((s) => ({ ...s, entries: s.entries.map((e) => (e.id === id ? next : e)) }))
	);
}

export function removeEntry(doc: NavDoc, id: string): NavDoc {
	return withSections(
		doc,
		doc.sections.map((s) => ({ ...s, entries: s.entries.filter((e) => e.id !== id) }))
	);
}

/** The built-ins of a scope that no entry uses yet (a built-in may appear once). */
export function freeBuiltins(doc: NavDoc, scope: NavScope, keep?: string): string[] {
	const used = new Set(
		doc.sections.flatMap((s) =>
			s.entries.flatMap((e) => (e.target.kind === 'builtin' ? [e.target.page] : []))
		)
	);
	return Object.keys(scope === 'workspace' ? WORKSPACE_BUILTINS : PROJECT_BUILTINS).filter(
		(page) => !used.has(page) || page === keep
	);
}

/** The target kinds an administrator can add in a scope. */
export function targetKinds(scope: NavScope): NavTarget['kind'][] {
	return scope === 'workspace'
		? ['builtin', 'project', 'form', 'flow_page', 'layout_page', 'external']
		: ['builtin', 'form', 'flow_page', 'layout_page', 'plugin_app', 'external'];
}

/** Whether a link is one the server accepts: http or https, a host, no userinfo, at most 2048. */
export function externalUrlOk(raw: string): boolean {
	if (raw.length > 2048 || raw.trim() !== raw) return false;
	try {
		const url = new URL(raw);
		return (
			(url.protocol === 'https:' || url.protocol === 'http:') &&
			url.username === '' &&
			url.password === '' &&
			url.hostname !== ''
		);
	} catch {
		return false;
	}
}

/** Trims a label map: empty texts are dropped; none left means "use the target's own name". */
export function cleanLabels(labels: LocaleLabels): LocaleLabels | null {
	const out: LocaleLabels = {};
	for (const [locale, text] of Object.entries(labels)) {
		const trimmed = text.trim().slice(0, MAX_LABEL);
		if (trimmed) out[locale] = trimmed;
	}
	return Object.keys(out).length > 0 ? out : null;
}

/** Whether two documents are the same (the editor's "unsaved changes"). */
export function sameDoc(a: NavDoc, b: NavDoc): boolean {
	return JSON.stringify(a) === JSON.stringify(b);
}
