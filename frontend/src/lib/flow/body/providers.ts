/** Byte-only authority: components never invent a Page ID or fetch a whole document as fallback. */
import { flowApi } from '#lib/api/flow.js';
import type { ApiResult } from '#lib/api/client.js';
import { transportFailure } from '#lib/api/transport.js';
export interface AssetBytesReader {
	key: string;
	read(assetId: string, signal?: AbortSignal): Promise<ApiResult<{ blob: Blob; headers: Headers }>>;
}
export interface BodyAssetProvider extends AssetBytesReader {
	mode: 'private' | 'public';
}

export function privateBodyAssetProvider(objectId: string): BodyAssetProvider {
	return {
		mode: 'private',
		key: objectId,
		read: (asset, signal) =>
			objectId
				? flowApi.bodyAsset(objectId, asset, signal)
				: Promise.resolve(transportFailure(400, 'transport.privateAssetUnavailable'))
	};
}
export function publicBodyAssetProvider(token: string): BodyAssetProvider {
	return {
		mode: 'public',
		key: token,
		read: (asset, signal) =>
			token
				? flowApi.publicBodyAsset(token, asset, signal)
				: Promise.resolve(transportFailure(400, 'transport.publicAssetUnavailable'))
	};
}
