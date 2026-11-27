/**
 * What a person can change in the navigation, as pure functions over `sv.nav.prefs.v1`. The
 * sheet works on a model built once from the resolved menu (loaded with `include_user_hidden`)
 * and the stored preferences; every change returns new preferences and the model is derived
 * from them again, so the sheet needs no reload between changes. The server applies the same
 * rules at resolution (a locked entry ignores every one of these).
 */
import { MAX_PINNED } from './limits';
import type { NavPrefs, ResolvedEntry, ResolvedNav } from './types';

export function emptyPrefs(): NavPrefs {
	return {
		schema: 'sv.nav.prefs.v1',
		hidden: [],
		order: {},
		pinned: [],
		unpinned: [],
		collapsed: [],
		expanded: []
	};
}

export function normalisePrefs(prefs: Partial<NavPrefs> | null | undefined): NavPrefs {
	return { ...emptyPrefs(), ...(prefs ?? {}), schema: 'sv.nav.prefs.v1' };
}

export interface SheetEntry {
	entry: ResolvedEntry;
	hidden: boolean;
	pinned: boolean;
}

export interface SheetSection {
	id: string;
	label: ResolvedNav['sections'][number]['label'];
	collapsed: boolean;
	entries: SheetEntry[];
}

const without = (list: readonly string[], id: string) => list.filter((x) => x !== id);
const withId = (list: readonly string[], id: string) =>
	list.includes(id) ? [...list] : [...list, id];

/** The entries of the menu grouped by the section they are stored in, in their loaded order. */
export function baseSections(
	menu: ResolvedNav
): Array<Omit<SheetSection, 'entries'> & { entries: ResolvedEntry[] }> {
	const order: string[] = [];
	const meta = new Map<string, ResolvedNav['sections'][number]>();
	const entries = new Map<string, ResolvedEntry[]>();
	for (const section of menu.sections) {
		if (section.id !== '_pinned') {
			if (!order.includes(section.id)) order.push(section.id);
			meta.set(section.id, section);
		}
		for (const entry of section.entries) {
			const list = entries.get(entry.section_id) ?? [];
			list.push(entry);
			entries.set(entry.section_id, list);
			if (!order.includes(entry.section_id)) order.push(entry.section_id);
		}
	}
	return order.map((id) => ({
		id,
		label: meta.get(id)?.label ?? null,
		collapsed: meta.get(id)?.collapsed ?? false,
		entries: (entries.get(id) ?? []).sort((a, b) => a.position - b.position)
	}));
}

/** Locked entries keep their index; the others follow `order`, unlisted ones after the listed. */
export function applyOrder<T extends { locked: boolean; id: string }>(
	items: readonly T[],
	order: readonly string[] | undefined
): T[] {
	if (!order) return [...items];
	const rank = (id: string) => order.indexOf(id);
	const movable = items.filter((i) => !i.locked);
	const listed = movable.filter((i) => rank(i.id) >= 0).sort((a, b) => rank(a.id) - rank(b.id));
	const queue = [...listed, ...movable.filter((i) => rank(i.id) < 0)];
	return items.map((item) => (item.locked ? item : (queue.shift() as T)));
}

/**
 * The sheet's view of the menu under `prefs`. `initial` are the preferences the menu was resolved
 * with, so an administrator's pin (pinned and not in `initial.pinned`) can be told from the
 * person's own.
 */
export function buildSections(
	menu: ResolvedNav,
	initial: NavPrefs,
	prefs: NavPrefs
): SheetSection[] {
	return baseSections(menu).map((section) => {
		const ordered = applyOrder(
			section.entries.map((e) => ({ id: e.id, locked: e.locked, entry: e })),
			prefs.order[section.id]
		).map((x) => x.entry);
		const collapsed = prefs.collapsed.includes(section.id)
			? true
			: prefs.expanded.includes(section.id)
				? false
				: section.collapsed;
		return {
			id: section.id,
			label: section.label,
			collapsed,
			entries: ordered.map((entry) => {
				const adminPinned = entry.pinned && !initial.pinned.includes(entry.id);
				const pinned = entry.locked
					? entry.pinned
					: prefs.pinned.includes(entry.id) || (adminPinned && !prefs.unpinned.includes(entry.id));
				return { entry, hidden: !entry.locked && prefs.hidden.includes(entry.id), pinned };
			})
		};
	});
}

export function setHidden(prefs: NavPrefs, entry: ResolvedEntry, hidden: boolean): NavPrefs {
	if (entry.locked) return prefs;
	return {
		...prefs,
		hidden: hidden ? withId(prefs.hidden, entry.id) : without(prefs.hidden, entry.id)
	};
}

/** Whether pinning one more entry would pass the cap (the server also cuts at the cap). */
export function pinCount(sections: readonly SheetSection[]): number {
	return sections.reduce((n, s) => n + s.entries.filter((e) => e.pinned && !e.hidden).length, 0);
}

export function setPinned(prefs: NavPrefs, entry: ResolvedEntry, pinned: boolean): NavPrefs {
	if (entry.locked) return prefs;
	return pinned
		? {
				...prefs,
				pinned: withId(prefs.pinned, entry.id),
				unpinned: without(prefs.unpinned, entry.id)
			}
		: {
				...prefs,
				pinned: without(prefs.pinned, entry.id),
				unpinned: withId(prefs.unpinned, entry.id)
			};
}

/** Writes the section's current order of its unlocked entries with `id` moved to `index` among them. */
export function moveWithin(
	prefs: NavPrefs,
	section: SheetSection,
	id: string,
	index: number
): NavPrefs {
	const movable = section.entries.filter((e) => !e.entry.locked).map((e) => e.entry.id);
	const from = movable.indexOf(id);
	if (from < 0) return prefs;
	movable.splice(from, 1);
	movable.splice(Math.max(0, Math.min(index, movable.length)), 0, id);
	return { ...prefs, order: { ...prefs.order, [section.id]: movable } };
}

/** One step among the unlocked entries (locked ones are jumped over). */
export function stepWithin(
	prefs: NavPrefs,
	section: SheetSection,
	id: string,
	direction: -1 | 1
): NavPrefs {
	const movable = section.entries.filter((e) => !e.entry.locked).map((e) => e.entry.id);
	const from = movable.indexOf(id);
	const to = from + direction;
	if (from < 0 || to < 0 || to >= movable.length) return prefs;
	return moveWithin(prefs, section, id, to);
}

export function setSectionCollapsed(
	prefs: NavPrefs,
	sectionId: string,
	collapsed: boolean
): NavPrefs {
	return collapsed
		? {
				...prefs,
				collapsed: withId(prefs.collapsed, sectionId),
				expanded: without(prefs.expanded, sectionId)
			}
		: {
				...prefs,
				collapsed: without(prefs.collapsed, sectionId),
				expanded: withId(prefs.expanded, sectionId)
			};
}

/** Whether the preferences differ from "no changes". */
export function isEmpty(prefs: NavPrefs): boolean {
	return JSON.stringify(prefs) === JSON.stringify(emptyPrefs());
}

export { MAX_PINNED };
