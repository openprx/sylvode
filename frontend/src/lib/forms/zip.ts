import type { ZipFileInput } from '#lib/forms/page-types.js';
import { isRecord } from '#lib/forms/record-format.js';

export function safeExportSlug(value: string): string {
	return (
		value
			.trim()
			.replace(/[^a-zA-Z0-9._-]+/g, '-')
			.replace(/^-+|-+$/g, '') || 'group'
	);
}

export function safeZipSegment(value: string): string {
	const safe = value
		.trim()
		.replaceAll('\\', '-')
		.replaceAll('/', '-')
		.replace(/[^a-zA-Z0-9._-]+/g, '-')
		.replace(/^-+|-+$/g, '');

	return safe || 'attachment.bin';
}

export function createStoredZipBlob(files: ZipFileInput[]): Blob {
	const encoder = new TextEncoder();
	const localParts: Uint8Array[] = [];
	const centralParts: Uint8Array[] = [];
	let offset = 0;

	for (const file of files) {
		const nameBytes = encoder.encode(file.path);
		const dataBytes = typeof file.data === 'string' ? encoder.encode(file.data) : file.data;
		const crc = crc32(dataBytes);
		const localHeader = new Uint8Array(30 + nameBytes.length);
		const localView = new DataView(localHeader.buffer);

		localView.setUint32(0, 0x04034b50, true);
		localView.setUint16(4, 20, true);
		localView.setUint16(6, 0, true);
		localView.setUint16(8, 0, true);
		localView.setUint16(10, 0, true);
		localView.setUint16(12, 0, true);
		localView.setUint32(14, crc, true);
		localView.setUint32(18, dataBytes.length, true);
		localView.setUint32(22, dataBytes.length, true);
		localView.setUint16(26, nameBytes.length, true);
		localView.setUint16(28, 0, true);
		localHeader.set(nameBytes, 30);
		localParts.push(localHeader, dataBytes);

		const centralHeader = new Uint8Array(46 + nameBytes.length);
		const centralView = new DataView(centralHeader.buffer);

		centralView.setUint32(0, 0x02014b50, true);
		centralView.setUint16(4, 20, true);
		centralView.setUint16(6, 20, true);
		centralView.setUint16(8, 0, true);
		centralView.setUint16(10, 0, true);
		centralView.setUint16(12, 0, true);
		centralView.setUint16(14, 0, true);
		centralView.setUint32(16, crc, true);
		centralView.setUint32(20, dataBytes.length, true);
		centralView.setUint32(24, dataBytes.length, true);
		centralView.setUint16(28, nameBytes.length, true);
		centralView.setUint16(30, 0, true);
		centralView.setUint16(32, 0, true);
		centralView.setUint16(34, 0, true);
		centralView.setUint16(36, 0, true);
		centralView.setUint32(38, 0, true);
		centralView.setUint32(42, offset, true);
		centralHeader.set(nameBytes, 46);
		centralParts.push(centralHeader);
		offset += localHeader.length + dataBytes.length;
	}

	const centralOffset = offset;
	const centralSize = centralParts.reduce((sum, part) => sum + part.length, 0);
	const end = new Uint8Array(22);
	const endView = new DataView(end.buffer);

	endView.setUint32(0, 0x06054b50, true);
	endView.setUint16(4, 0, true);
	endView.setUint16(6, 0, true);
	endView.setUint16(8, files.length, true);
	endView.setUint16(10, files.length, true);
	endView.setUint32(12, centralSize, true);
	endView.setUint32(16, centralOffset, true);
	endView.setUint16(20, 0, true);

	return new Blob([...localParts, ...centralParts, end].map(zipBlobPart), {
		type: 'application/zip'
	});
}

export function zipBlobPart(bytes: Uint8Array): ArrayBuffer {
	const copy = new Uint8Array(bytes.byteLength);

	copy.set(bytes);

	return copy.buffer;
}

const crc32Table = Array.from({ length: 256 }, (_, index) => {
	let value = index;

	for (let bit = 0; bit < 8; bit += 1) {
		value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
	}

	return value >>> 0;
});

export function crc32(bytes: Uint8Array): number {
	let crc = 0xffffffff;

	for (const byte of bytes) {
		crc = crc32Table[(crc ^ byte) & 0xff] ^ (crc >>> 8);
	}

	return (crc ^ 0xffffffff) >>> 0;
}

export function attachmentManifestUrls(value: unknown): string[] {
	if (typeof value === 'string') return value.trim() ? [value.trim()] : [];

	if (Array.isArray(value)) {
		return value.flatMap((item) => attachmentManifestUrls(item));
	}

	if (isRecord(value)) {
		if (typeof value.url === 'string') return attachmentManifestUrls(value.url);
		if (typeof value.href === 'string') return attachmentManifestUrls(value.href);
	}

	return [];
}
