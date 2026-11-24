/** Share-only byte authority, locally captured aliases and bounded request lifetime. No Page fallback. */
import type { ApiResult } from '#lib/api/client.js';
import { transportFailure } from '#lib/api/transport.js';
import type { SharedRead } from './contracts';
import { parseSharedRead } from './contracts';
import { sharedBlockApi } from './client';
export interface SharedAssetProvider {
	mode: 'shared';
	key: string;
	read(alias: string, signal?: AbortSignal): Promise<ApiResult<{ blob: Blob; headers: Headers }>>;
	dispose(): void;
}
export function sharedAssetProvider(
	read: SharedRead,
	authorized: () => boolean = () => true
): SharedAssetProvider {
	const { projection } = parseSharedRead(read);
	const aliases = new Set(
		projection.members.flatMap((member) =>
			'asset_alias' in member.content ? [member.content.asset_alias] : []
		)
	);
	const pending = new Set<AbortController>();
	let live = true;
	const unavailable = (): ApiResult<{ blob: Blob; headers: Headers }> =>
		transportFailure(403, 'transport.sharedBlockUnavailable');
	return {
		mode: 'shared',
		key: `${projection.share_id}:${projection.revision}`,
		async read(alias, signal) {
			if (!live || !authorized() || signal?.aborted || !aliases.has(alias)) return unavailable();
			const controller = new AbortController(),
				abort = () => controller.abort();
			pending.add(controller);
			signal?.addEventListener('abort', abort, { once: true });
			try {
				const result = await sharedBlockApi.asset(projection.share_id, alias, controller.signal);
				if (!live || !authorized() || controller.signal.aborted) return unavailable();
				if (result.code !== 0 || !result.data) return result;
				if (
					result.data.headers.get('x-flow-share-id') !== projection.share_id ||
					result.data.headers.get('x-flow-asset-alias') !== alias
				)
					return transportFailure(502, 'transport.sharedBlockInvalid');
				return result;
			} finally {
				pending.delete(controller);
				signal?.removeEventListener('abort', abort);
			}
		},
		dispose() {
			live = false;
			for (const controller of pending) controller.abort();
			pending.clear();
		}
	};
}
