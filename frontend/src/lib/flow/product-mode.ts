/** Capability-envelope compatibility; distinct from the Page editor model (1/4/5). */
export const FLOW_PROCESS_READER_COMPATIBILITY = 5;
// Temporary product scope: retain typed data/services, expose only Page work in Flow UI.
export const FLOW_COLLECTION_UI_ENABLED = false;
export function pageOnlyParent(
	objectId: string,
	parents: ReadonlyMap<string, string | null>,
	pages: ReadonlySet<string>,
	root: string
): string {
	let parent = parents.get(objectId);
	const visited = new Set<string>([objectId]);
	while (parent && parent !== root) {
		if (pages.has(parent)) return parent;
		if (visited.has(parent)) return root;
		visited.add(parent);
		parent = parents.get(parent);
	}
	return root;
}

/** Presentation only: never rewrite the canonical parent or persist a structural move. */
export function pageOnlyParents(
	objects: readonly { id: string; parent_id?: string | null }[],
	hiddenParents: Record<string, string | null>,
	root: string
): ReadonlyMap<string, string> {
	const parents = new Map(Object.entries(hiddenParents));
	for (const object of objects) parents.set(object.id, object.parent_id ?? null);
	const pages = new Set(objects.map((object) => object.id));
	return new Map(
		objects.map((object) => [object.id, pageOnlyParent(object.id, parents, pages, root)])
	);
}
