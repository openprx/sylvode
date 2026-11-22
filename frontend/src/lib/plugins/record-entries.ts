import type { Writable } from 'svelte/store';
import type { PluginContributionItem, PluginPlacement, PluginSlotV3 } from '../api/plugin-packages';

/** A record contribution an actor can launch. `position` is the effective (personal or project) position. */
export interface RecordEntry {
	item: PluginContributionItem;
	id: string;
	label: string;
	slot: PluginSlotV3;
	position: number;
}

export const entryPluginKey = (entry: RecordEntry): string => entry.item.key;

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

/** Record-slot entries of contributions, excluding personally hidden ones, in launch order. */
export function contributionEntries(
	items: PluginContributionItem[],
	slot: PluginSlotV3
): RecordEntry[] {
	return items
		.filter((item) => item.contribution.slot === slot && !item.preference.hidden)
		.map((item) => ({
			item,
			id: item.contribution.id,
			label: item.contribution.label,
			slot,
			position: item.preference.position ?? item.project_position
		}))
		.sort(
			(left, right) =>
				left.position - right.position ||
				compare(entryPluginKey(left), entryPluginKey(right)) ||
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

export function validPlacementPositions(placements: PluginPlacement[] | undefined): boolean {
	return (
		placements?.every(
			(placement) =>
				Number.isInteger(placement.position) && placement.position >= 0 && placement.position <= 99
		) ?? true
	);
}
