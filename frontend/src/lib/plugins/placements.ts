import type { Writable } from 'svelte/store';
import type {
	InstalledPluginPackage,
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

export type RecordPluginActionSurface = Writable<{
	recordId: string;
	panelId: string;
	entries: RecordPluginEntry[];
	active: RecordPluginEntry | null;
	toggle: (entry: RecordPluginEntry, trigger: HTMLElement) => void;
} | null>;

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

export function sameEntry(first: RecordPluginEntry | null, second: RecordPluginEntry): boolean {
	return first?.panel.key === second.panel.key && first.id === second.id;
}

export function placementDraft(item: InstalledPluginPackage): PluginPlacement[] | undefined {
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
