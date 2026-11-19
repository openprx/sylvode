import type { Writable } from 'svelte/store';
import { isV3 } from '../api/plugin-packages';
import type {
	InstalledPluginPackage,
	PluginContributionItem,
	PluginContribution,
	PluginPanel,
	PluginPlacement,
	PluginSlot
} from '../api/plugin-packages';

export interface RecordPluginEntry {
	panel: PluginPanel;
	id: string;
	label: string;
	slot: PluginSlot;
	position: number;
}

/** A protocol 3 record contribution. `position` is the effective (personal or project) position. */
export interface RecordContributionEntry {
	item: PluginContributionItem;
	id: string;
	label: string;
	slot: PluginSlot;
	position: number;
}

/** Any entry a record can launch: protocol 1/2 (`panel`) or protocol 3 (`item`). */
export type RecordEntry = RecordPluginEntry | RecordContributionEntry;

export const isContributionEntry = (entry: RecordEntry): entry is RecordContributionEntry =>
	'item' in entry;

export const entryPluginKey = (entry: RecordEntry): string =>
	isContributionEntry(entry) ? entry.item.key : entry.panel.key;

export interface RecordActionSurfaceEntry {
	recordId: string;
	panelId: string;
	entries: RecordEntry[];
	/** Entries moved into the "More" menu (personal collapse or beyond the inline limit). */
	more: RecordEntry[];
	active: RecordEntry | null;
	toggle: (entry: RecordEntry, trigger: HTMLElement) => void;
}

export type RecordPluginActionSurface = Writable<RecordActionSurfaceEntry | null>;

/** Record-slot entries of protocol 3 contributions, excluding personally hidden ones. */
export function contributionEntries(
	items: PluginContributionItem[],
	slot: PluginSlot
): RecordContributionEntry[] {
	return items
		.filter((item) => item.contribution.slot === slot && !item.preference.hidden)
		.map((item) => ({
			item,
			id: item.contribution.id,
			label: item.contribution.label,
			slot,
			position: item.preference.position ?? item.project_position
		}));
}

/** Orders legacy and protocol 3 entries together: (position, plugin key, contribution id). */
export function mergeEntries(
	legacy: RecordPluginEntry[],
	contributions: RecordContributionEntry[]
): RecordEntry[] {
	return [...legacy, ...contributions].sort(
		(left, right) =>
			left.position - right.position ||
			compare(entryPluginKey(left), entryPluginKey(right)) ||
			compare(left.id, right.id)
	);
}

export function panelEntries(panels: PluginPanel[], slot: PluginSlot): RecordPluginEntry[] {
	const entries = panels.flatMap((panel) => {
		const contributions = panel.contributions ?? [
			{
				id: 'detail',
				slot: 'form.record.detail',
				label: panel.name,
				position: 50
			}
		];
		return contributions
			.filter((entry) => entry.slot === slot)
			.map((entry) => ({
				panel,
				id: entry.id,
				slot,
				label: entry.label,
				position: entry.position
			}));
	});
	return entries.sort(
		(left, right) =>
			left.position - right.position ||
			compare(left.panel.key, right.panel.key) ||
			compare(left.id, right.id)
	);
}

function compare(first: string, second: string) {
	if (first === second) return 0;
	return first < second ? -1 : 1;
}

export function sameEntry(first: RecordEntry | null, second: RecordEntry): boolean {
	return !!first && entryPluginKey(first) === entryPluginKey(second) && first.id === second.id;
}

export function placementDraft(item: InstalledPluginPackage): PluginPlacement[] | undefined {
	if (isV3(item.manifest)) return undefined;
	return item.manifest.ui.contributions?.map((contribution) => {
		const existing = item.placements?.find((placement) => placement.id === contribution.id);
		return {
			id: contribution.id,
			enabled: existing?.enabled ?? true,
			position: existing?.position ?? 50
		};
	});
}

export function isContribution(value: unknown): value is PluginContribution {
	if (value === null || typeof value !== 'object') return false;
	const item = value as Partial<PluginContribution>;
	return (
		typeof item.id === 'string' &&
		typeof item.label === 'string' &&
		(item.slot === 'form.record.detail' || item.slot === 'form.record.actions')
	);
}

export function validPlacementPositions(placements: PluginPlacement[] | undefined): boolean {
	return (
		placements?.every(
			(placement) =>
				Number.isInteger(placement.position) && placement.position >= 0 && placement.position <= 99
		) ?? true
	);
}
