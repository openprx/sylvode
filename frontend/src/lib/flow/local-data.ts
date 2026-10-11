// Per-user Flow local databases (`ui-surface-v1.md` "职责边界": the IndexedDB accepted cache and
// local outbox belong to `ObjectRepository`).
//
// Every Flow IndexedDB database carries the signed-in user's id in its NAME
// (`sylvode-flow-outbox-v1:<userId>`, `sylvode-flow-cache-v1:<userId>`) rather than a `userId`
// field plus an index on shared stores. The name is the isolation boundary: no read path -- a
// `getAll()`, an index lookup, a draft listing, `nextOrder()` -- can reach another user's rows,
// even one written by a future query that forgot a filter, and a user who left without logging
// out (closed tab, expired session) finds their own pending intents again on their next sign-in,
// while anyone else signing in on the same browser opens a different database. Without a known
// user nothing is persisted at all (the outbox falls back to memory, the cache to nothing).
//
// Logout deletes every Flow database of this origin (`clearFlowLocalData`), including the
// pre-isolation global ones.

export const OUTBOX_DB_PREFIX = 'sylvode-flow-outbox-v1';
export const CACHE_DB_PREFIX = 'sylvode-flow-cache-v1';
const FLOW_DB_PREFIXES = [OUTBOX_DB_PREFIX, CACHE_DB_PREFIX] as const;

/** The database name of one user's store, or `null` without a user (nothing is persisted). */
export function flowDatabaseName(prefix: string, userId: string | null | undefined): string | null {
	if (typeof userId !== 'string' || userId.length === 0) return null;
	return `${prefix}:${userId}`;
}

function isFlowDatabase(name: string): boolean {
	return FLOW_DB_PREFIXES.some((prefix) => name === prefix || name.startsWith(`${prefix}:`));
}

function deleteDatabase(factory: IDBFactory, name: string): Promise<void> {
	return new Promise((resolve) => {
		let request: IDBOpenDBRequest;
		try {
			request = factory.deleteDatabase(name);
		} catch {
			resolve();
			return;
		}
		request.onsuccess = () => resolve();
		request.onerror = () => resolve();
		// Open connections are closed by their `onversionchange` handlers; `blocked` only means
		// the delete is queued behind them and still completes.
	});
}

/**
 * Deletes every Flow IndexedDB database of this origin: each user's outbox (with its recovery
 * drafts) and snapshot cache, and the global pre-isolation ones. `knownUserIds` covers browsers
 * without `indexedDB.databases()`; with it, every Flow database present is deleted.
 */
export async function clearFlowLocalData(
	knownUserIds: readonly string[] = [],
	factory: IDBFactory | undefined = typeof indexedDB === 'undefined' ? undefined : indexedDB
): Promise<void> {
	if (!factory) return;
	const names = new Set<string>(FLOW_DB_PREFIXES);
	for (const userId of knownUserIds) {
		for (const prefix of FLOW_DB_PREFIXES) {
			const name = flowDatabaseName(prefix, userId);
			if (name) names.add(name);
		}
	}
	if (typeof factory.databases === 'function') {
		try {
			for (const info of await factory.databases()) {
				if (info.name && isFlowDatabase(info.name)) names.add(info.name);
			}
		} catch {
			// The listing is best effort; the known names are still deleted.
		}
	}
	await Promise.all([...names].map((name) => deleteDatabase(factory, name)));
}
