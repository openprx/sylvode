import type { Destination, RouteContext } from './route-map';
import {
	browserStorage,
	readJson,
	safeInAppPath,
	shortcutsKey,
	writeJson,
	type StorageLike
} from './storage';

export type ShortcutKind = 'project' | 'form' | 'flow-page' | 'view' | 'governance';

export interface Shortcut {
	/** Stable: `${kind}:${objectId}` ('governance:governance'). */
	id: string;
	kind: ShortcutKind;
	/** Cached display name, refreshed when the object is seen again. */
	label: string;
	/** In-app path. */
	href: string;
	workspaceId?: string;
	projectId?: string;
	formId?: string;
	objectId?: string;
}

export interface ShortcutsFile {
	version: 1;
	items: Shortcut[];
}

export type ShortcutAvailability =
	| { state: 'unknown' | 'available' }
	/** `reason` is an i18n key. */
	| { state: 'unavailable'; reason: string };

export const MAX_SHORTCUTS = 8;
export const SHORTCUT_UNAVAILABLE_REASON = 'shell.shortcuts.unavailable';
export const SHORTCUT_FULL_REASON = 'shell.shortcuts.full';

const KINDS: readonly ShortcutKind[] = ['project', 'form', 'flow-page', 'view', 'governance'];

/** Glyph (Lucide) of each shortcut kind. */
export const SHORTCUT_GLYPHS: Record<ShortcutKind, string> = {
	project: 'folder',
	form: 'table-2',
	'flow-page': 'file-text',
	view: 'layout-list',
	governance: 'landmark'
};

export function shortcutId(kind: ShortcutKind, objectId: string): string {
	return `${kind}:${objectId}`;
}

/** The Governance shortcut the editor offers. */
export function governanceShortcut(label: string): Shortcut {
	return {
		id: shortcutId('governance', 'governance'),
		kind: 'governance',
		label,
		href: '/governance',
		objectId: 'governance'
	};
}

function optionalString(value: unknown): string | undefined {
	return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function parseShortcut(raw: unknown): Shortcut | null {
	if (typeof raw !== 'object' || raw === null) return null;
	const r = raw as Record<string, unknown>;
	const kind = KINDS.find((k) => k === r.kind);
	const href = safeInAppPath(r.href);
	if (!kind || !href || typeof r.id !== 'string' || !r.id || typeof r.label !== 'string')
		return null;
	return {
		id: r.id,
		kind,
		label: r.label.slice(0, 200),
		href,
		workspaceId: optionalString(r.workspaceId),
		projectId: optionalString(r.projectId),
		formId: optionalString(r.formId),
		objectId: optionalString(r.objectId)
	};
}

/** Valid, de-duplicated items of a stored file (at most 8); anything else yields an empty list. */
export function parseShortcutsFile(raw: unknown): Shortcut[] {
	if (typeof raw !== 'object' || raw === null) return [];
	const file = raw as Record<string, unknown>;
	if (file.version !== 1 || !Array.isArray(file.items)) return [];
	const items: Shortcut[] = [];
	for (const entry of file.items) {
		const item = parseShortcut(entry);
		if (!item || items.some((i) => i.id === item.id)) continue;
		items.push(item);
		if (items.length >= MAX_SHORTCUTS) break;
	}
	return items;
}

/** The destination a shortcut belongs to. */
export function shortcutDestination(kind: ShortcutKind): Destination {
	switch (kind) {
		case 'flow-page':
			return 'flow';
		case 'governance':
			return 'governance';
		default:
			return 'projects';
	}
}

/**
 * The shortcut selected by a URL: the one whose path the pathname starts with (at a segment
 * boundary); a form shortcut also needs its `?form=`. The longest path wins; on a tie the form
 * shortcut wins.
 */
export function selectedShortcutId(items: readonly Shortcut[], url: URL): string | null {
	let best: { id: string; length: number; specific: boolean } | null = null;
	for (const item of items) {
		const path = item.href.split('?')[0];
		const boundary = path === '/' || url.pathname === path || url.pathname.startsWith(path + '/');
		if (!boundary) continue;
		if (item.formId && url.searchParams.get('form') !== item.formId) continue;
		const candidate = { id: item.id, length: path.length, specific: Boolean(item.formId) };
		if (
			!best ||
			candidate.length > best.length ||
			(candidate.length === best.length && candidate.specific && !best.specific)
		)
			best = candidate;
	}
	return best?.id ?? null;
}

export interface RailSelection {
	/** Destination whose column is shown; null in settings mode. */
	column: Destination | null;
	/** Destination item the rail marks selected. A selected shortcut takes the mark from its destination. */
	destination: Destination | null;
	shortcutId: string | null;
	settings: boolean;
}

/** What the rail and the column show as selected for a URL. */
export function railSelection(
	ctx: RouteContext,
	url: URL,
	items: readonly Shortcut[]
): RailSelection {
	if (ctx.settings) return { column: null, destination: null, shortcutId: null, settings: true };
	const shortcutId = selectedShortcutId(items, url);
	return {
		column: ctx.destination,
		destination: shortcutId ? null : ctx.destination,
		shortcutId,
		settings: false
	};
}

/** `ok` false carries the i18n key of the reason, shown inline in the editor (never a toast). */
export type AddResult = { ok: true; added: boolean } | { ok: false; reason: string };

/** Outcome of one availability check: 'gone' is HTTP 403 or 404; 'error' leaves the state unknown. */
export type AvailabilityCheck = 'available' | 'gone' | 'error';

/** The user's rail shortcuts: at most eight, stored per user in the browser. */
export class RailShortcuts {
	readonly userId: string | null;
	private readonly storage: StorageLike | null;
	items = $state.raw<Shortcut[]>([]);
	availability = $state.raw<Record<string, ShortcutAvailability>>({});
	private validated = false;

	constructor(options: { userId: string | null; storage?: StorageLike | null }) {
		this.userId = options.userId;
		this.storage = options.userId ? (options.storage ?? browserStorage('local')) : null;
		if (options.userId)
			this.items = parseShortcutsFile(readJson(this.storage, shortcutsKey(options.userId)));
	}

	has(id: string): boolean {
		return this.items.some((item) => item.id === id);
	}

	/** Adding an existing id changes nothing (not even its position); label and href refresh. */
	add(shortcut: Shortcut): AddResult {
		const existing = this.items.findIndex((item) => item.id === shortcut.id);
		if (existing >= 0) {
			this.refresh(shortcut);
			return { ok: true, added: false };
		}
		if (this.items.length >= MAX_SHORTCUTS) return { ok: false, reason: SHORTCUT_FULL_REASON };
		const clean = parseShortcut(shortcut);
		if (!clean) return { ok: false, reason: SHORTCUT_UNAVAILABLE_REASON };
		this.items = [...this.items, clean];
		this.persist();
		return { ok: true, added: true };
	}

	/** Updates the cached label and href of a shortcut seen again. */
	refresh(shortcut: Pick<Shortcut, 'id' | 'label' | 'href'>): void {
		const href = safeInAppPath(shortcut.href);
		const index = this.items.findIndex((item) => item.id === shortcut.id);
		if (index < 0 || !href) return;
		const current = this.items[index];
		if (current.label === shortcut.label && current.href === href) return;
		this.items = this.items.map((item, i) =>
			i === index ? { ...item, label: shortcut.label.slice(0, 200), href } : item
		);
		this.persist();
	}

	remove(id: string): void {
		if (!this.has(id)) return;
		this.items = this.items.filter((item) => item.id !== id);
		const { [id]: _gone, ...rest } = this.availability;
		void _gone;
		this.availability = rest;
		this.persist();
	}

	/** Moves one place up (-1) or down (+1). Returns false at the ends. */
	move(id: string, delta: -1 | 1): boolean {
		const from = this.items.findIndex((item) => item.id === id);
		const to = from + delta;
		if (from < 0 || to < 0 || to >= this.items.length) return false;
		const next = [...this.items];
		[next[from], next[to]] = [next[to], next[from]];
		this.items = next;
		this.persist();
		return true;
	}

	/** "Restore default navigation": removes every shortcut. */
	restoreDefault(): void {
		if (this.items.length === 0) return;
		this.items = [];
		this.availability = {};
		this.persist();
	}

	availabilityOf(id: string): ShortcutAvailability {
		return this.availability[id] ?? { state: 'unknown' };
	}

	/**
	 * Checks every shortcut once per session, at most `concurrency` at a time. Governance is
	 * always available. Only 'gone' marks a shortcut unavailable; a failed check stays unknown.
	 */
	async validate(
		check: (shortcut: Shortcut) => Promise<AvailabilityCheck>,
		concurrency = 2
	): Promise<void> {
		if (this.validated) return;
		this.validated = true;
		const queue = this.items.filter((item) => {
			if (item.kind !== 'governance') return true;
			this.setAvailability(item.id, { state: 'available' });
			return false;
		});
		const worker = async () => {
			for (let item = queue.shift(); item; item = queue.shift()) {
				let outcome: AvailabilityCheck;
				try {
					outcome = await check(item);
				} catch {
					outcome = 'error';
				}
				if (!this.has(item.id)) continue;
				if (outcome === 'available') this.setAvailability(item.id, { state: 'available' });
				else if (outcome === 'gone')
					this.setAvailability(item.id, {
						state: 'unavailable',
						reason: SHORTCUT_UNAVAILABLE_REASON
					});
			}
		};
		await Promise.all(Array.from({ length: Math.max(1, concurrency) }, worker));
	}

	private setAvailability(id: string, value: ShortcutAvailability): void {
		this.availability = { ...this.availability, [id]: value };
	}

	private persist(): void {
		if (!this.userId) return;
		const file: ShortcutsFile = { version: 1, items: this.items };
		writeJson(this.storage, shortcutsKey(this.userId), file);
	}
}
