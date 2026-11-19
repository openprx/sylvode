import {
	isV3,
	type InstalledPluginPackage,
	type PluginBinding,
	type PluginGrant,
	type PluginManifestV3,
	type PluginPlacement
} from '../api/plugin-packages';

export const DEFAULT_POSITION = 50;
export const UI_DISPLAY = 'ui.display';

const byId = <T extends { id: string }>(first: T, second: T) =>
	first.id < second.id ? -1 : first.id > second.id ? 1 : 0;

/**
 * The editable layout for a v3 package: exactly one row per declared contribution, sorted by id.
 * Stored rows for contributions this version does not declare are never submitted; the server keeps
 * them for a later rollback.
 */
export function layoutDraftV3(
	manifest: PluginManifestV3,
	stored: PluginPlacement[] | undefined
): PluginPlacement[] {
	return manifest.contributions
		.map((contribution) => {
			const row = stored?.find((placement) => placement.id === contribution.id);
			return {
				id: contribution.id,
				enabled: row?.enabled ?? true,
				position: row?.position ?? DEFAULT_POSITION
			};
		})
		.sort(byId);
}

/** Layout draft for any installed package, or undefined for protocols that have no v3 layout. */
export function installedLayoutDraftV3(
	item: InstalledPluginPackage,
	binding?: PluginBinding
): PluginPlacement[] | undefined {
	if (!isV3(item.manifest)) return undefined;
	return layoutDraftV3(
		item.manifest,
		binding?.package_id === item.id && binding.placements ? binding.placements : item.placements
	);
}

export interface GrantOptionV3 {
	grant: PluginGrant;
	kind: 'display' | 'read' | 'capability';
	read?: { id: string; form_key: string; fields: string[] };
}

export function grantOptionsV3(manifest: PluginManifestV3): GrantOptionV3[] {
	return [
		{ grant: UI_DISPLAY, kind: 'display' },
		...manifest.reads.map((read) => ({
			grant: `read:${read.id}` as PluginGrant,
			kind: 'read' as const,
			read
		})),
		...manifest.capabilities.map((capability) => ({
			grant: capability.name,
			kind: 'capability' as const
		}))
	];
}

/** Sorted, de-duplicated and limited to grants this manifest can receive. */
export function normalizeGrantsV3(
	manifest: PluginManifestV3,
	grants: PluginGrant[]
): PluginGrant[] {
	const allowed = new Set(grantOptionsV3(manifest).map((option) => option.grant));
	return [...new Set(grants)].filter((grant) => allowed.has(grant)).sort();
}

/** Client mirror of the server grants rules 2 to 4 (rule 1 is handled by normalizing). */
export function grantsValidV3(manifest: PluginManifestV3, grants: PluginGrant[]): boolean {
	if (normalizeGrantsV3(manifest, grants).length !== grants.length) return false;
	if (grants.length && !grants.includes(UI_DISPLAY)) return false;
	return !grants.includes('amount.apply') || grants.includes('amount.preview');
}

/** Toggle one grant and keep the dependent grants consistent. */
export function toggleGrantV3(
	manifest: PluginManifestV3,
	grants: PluginGrant[],
	grant: PluginGrant,
	enabled: boolean
): PluginGrant[] {
	let next = new Set(grants);
	if (enabled) {
		next.add(grant);
		if (grant !== UI_DISPLAY) next.add(UI_DISPLAY);
		if (grant === 'amount.apply') next.add('amount.preview');
	} else {
		next.delete(grant);
		if (grant === UI_DISPLAY) next = new Set();
		if (grant === 'amount.preview') next.delete('amount.apply');
	}
	return normalizeGrantsV3(manifest, [...next]);
}
