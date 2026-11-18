/** Byte-only authority: components never invent a Page ID or fetch a whole document as fallback. */
import { flowApi } from '#lib/api/flow.js';
import type { ApiResult } from '#lib/api/client.js';
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
				: Promise.resolve({
						code: 400,
						message: 'A private asset provider is unavailable',
						data: null
					})
	};
}
export function publicBodyAssetProvider(token: string): BodyAssetProvider {
	return {
		mode: 'public',
		key: token,
		read: (asset, signal) =>
			token
				? flowApi.publicBodyAsset(token, asset, signal)
				: Promise.resolve({
						code: 400,
						message: 'A public asset provider is unavailable',
						data: null
					})
	};
}
