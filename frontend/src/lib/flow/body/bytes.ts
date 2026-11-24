import { transportFailure } from '#lib/api/transport.js';
/** Server-owned MIME/disposition decides rendering and downloading, never atom display labels. */
export const BODY_IMAGE_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif']);
export function bodyBytesValid(
	kind: 'image' | 'attachment',
	blob: Blob,
	headers: Headers
): boolean {
	const mime = (headers.get('content-type') ?? '').split(';')[0].trim().toLowerCase();
	if (
		!/^[a-f0-9]{64}$/i.test(headers.get('x-flow-asset-sha256') ?? '') ||
		headers.get('x-content-type-options')?.toLowerCase() !== 'nosniff'
	)
		return false;
	return (
		blob.size > 0 &&
		(kind === 'image'
			? BODY_IMAGE_TYPES.has(mime)
			: mime === 'application/octet-stream' &&
				/^attachment(?:;|$)/i.test(headers.get('content-disposition') ?? ''))
	);
}
export function bodyDownloadName(headers: Headers): string {
	const value = headers.get('content-disposition') ?? '';
	const encoded = /filename\*=UTF-8''([^;]+)/i.exec(value);
	const quoted = /filename="([^"\r\n]*)"/i.exec(value);
	let name = 'attachment';
	try {
		name = encoded ? decodeURIComponent(encoded[1]) : (quoted?.[1] ?? name);
	} catch {
		/* Invalid header falls back to a fixed safe name. */
	}
	return (
		Array.from(name, (char) => {
			const code = char.codePointAt(0)!;
			return code < 32 || code === 127 || char === '/' || char === '\\' ? '_' : char;
		})
			.join('')
			.slice(0, 255) || 'attachment'
	);
}

/** A successful body response needs the asset contract, not merely HTTP 200. */
export function bodyBinaryResult(
	result: import('#lib/api/client.js').ApiResult<{ blob: Blob; headers: Headers }>
): import('#lib/api/client.js').ApiResult<{ blob: Blob; headers: Headers }> {
	if (result.code !== 0 || !result.data) return result;
	const { blob, headers } = result.data;
	const mime = (headers.get('content-type') ?? '').split(';')[0].trim().toLowerCase();
	const kind = BODY_IMAGE_TYPES.has(mime) ? 'image' : 'attachment';
	return bodyBytesValid(kind, blob, headers)
		? result
		: transportFailure(502, 'transport.assetResponseInvalid');
}
