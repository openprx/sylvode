/**
 * Stored (uncompressed) zip writer, CRC-32, manifest URLs and file-name slugs of the forms page.
 * The archive bytes are parsed here with an independent reader (end-of-central-directory record,
 * central directory, local headers) instead of being read back through the module.
 *
 * Run standalone: `bun tests/forms-page/zip.test.ts`
 */
import { Suite, assert, assertDeepEqual, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';

await installSvelteSsr();
const zip = await import('../../src/lib/forms/zip');

const suite = new Suite('forms-page/zip');
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

interface ParsedEntry {
	name: string;
	crc: number;
	compressedSize: number;
	size: number;
	method: number;
	flags: number;
	localOffset: number;
	localHeaderCrc: number;
	localHeaderSize: number;
	localHeaderName: string;
	data: Uint8Array;
}

interface ParsedZip {
	entries: ParsedEntry[];
	endEntriesOnDisk: number;
	endEntriesTotal: number;
	centralSize: number;
	centralOffset: number;
	commentLength: number;
	length: number;
	centralSignatures: number[];
	firstSignature: number;
}

/** Reads the archive the way an unzip tool does: from the end-of-central-directory record. */
function parseZip(bytes: Uint8Array): ParsedZip {
	const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	const endAt = bytes.length - 22;
	assertEqual(view.getUint32(endAt, true), 0x06054b50, 'end record signature at length - 22');

	const endEntriesOnDisk = view.getUint16(endAt + 8, true);
	const endEntriesTotal = view.getUint16(endAt + 10, true);
	const centralSize = view.getUint32(endAt + 12, true);
	const centralOffset = view.getUint32(endAt + 16, true);
	const commentLength = view.getUint16(endAt + 20, true);
	assertEqual(
		centralOffset + centralSize,
		endAt,
		'central directory ends where the end record starts'
	);

	const entries: ParsedEntry[] = [];
	const centralSignatures: number[] = [];
	let cursor = centralOffset;
	for (let index = 0; index < endEntriesTotal; index += 1) {
		centralSignatures.push(view.getUint32(cursor, true));
		const flags = view.getUint16(cursor + 8, true);
		const method = view.getUint16(cursor + 10, true);
		const crc = view.getUint32(cursor + 16, true);
		const compressedSize = view.getUint32(cursor + 20, true);
		const size = view.getUint32(cursor + 24, true);
		const nameLength = view.getUint16(cursor + 28, true);
		const extraLength = view.getUint16(cursor + 30, true);
		const commentLen = view.getUint16(cursor + 32, true);
		const localOffset = view.getUint32(cursor + 42, true);
		const name = decoder.decode(bytes.subarray(cursor + 46, cursor + 46 + nameLength));

		assertEqual(view.getUint32(localOffset, true), 0x04034b50, `local signature of ${name}`);
		const localNameLength = view.getUint16(localOffset + 26, true);
		const localExtraLength = view.getUint16(localOffset + 28, true);
		const dataStart = localOffset + 30 + localNameLength + localExtraLength;

		entries.push({
			name,
			crc,
			compressedSize,
			size,
			method,
			flags,
			localOffset,
			localHeaderCrc: view.getUint32(localOffset + 14, true),
			localHeaderSize: view.getUint32(localOffset + 22, true),
			localHeaderName: decoder.decode(
				bytes.subarray(localOffset + 30, localOffset + 30 + localNameLength)
			),
			data: bytes.subarray(dataStart, dataStart + size)
		});
		cursor += 46 + nameLength + extraLength + commentLen;
	}
	assertEqual(cursor, centralOffset + centralSize, 'central records fill the central directory');

	return {
		entries,
		endEntriesOnDisk,
		endEntriesTotal,
		centralSize,
		centralOffset,
		commentLength,
		length: bytes.length,
		centralSignatures,
		firstSignature: view.getUint32(0, true)
	};
}

async function build(files: Parameters<typeof zip.createStoredZipBlob>[0]): Promise<{
	blob: Blob;
	bytes: Uint8Array;
	parsed: ParsedZip;
}> {
	const blob = zip.createStoredZipBlob(files);
	const bytes = new Uint8Array(await blob.arrayBuffer());
	return { blob, bytes, parsed: parseZip(bytes) };
}

suite.check('crc32 matches the published check values', () => {
	assertEqual(zip.crc32(encoder.encode('123456789')), 0xcbf43926, 'check string 123456789');
	assertEqual(zip.crc32(new Uint8Array(0)), 0, 'empty input');
	assertEqual(zip.crc32(encoder.encode('a')), 0xe8b7be43, 'a single letter');
	assertEqual(zip.crc32(encoder.encode('hello')), 0x3610a686, 'hello');
	assertEqual(
		zip.crc32(encoder.encode('The quick brown fox jumps over the lazy dog')),
		0x414fa339,
		'pangram'
	);
	assertEqual(zip.crc32(new Uint8Array([0])), 0xd202ef8d, 'one zero byte');
	assertEqual(zip.crc32(new Uint8Array(4).fill(0xff)), 0xffffffff, 'four 0xff bytes');
});

suite.check(
	'crc32 returns an unsigned 32-bit integer and does not depend on the view offset',
	() => {
		const value = zip.crc32(encoder.encode('hello'));
		assert(value >= 0 && value <= 0xffffffff && Number.isInteger(value), 'unsigned integer');
		const padded = encoder.encode('xxhelloyy');
		assertEqual(zip.crc32(padded.subarray(2, 7)), 0x3610a686, 'a subarray view of hello');
	}
);

await suite.checkAsync(
	'createStoredZipBlob writes an archive with the zip signatures and counts',
	async () => {
		const { blob, bytes, parsed } = await build([
			{ path: 'a.txt', data: '123456789' },
			{ path: 'dir/b.bin', data: new Uint8Array([1, 2, 3]) },
			{ path: 'empty.txt', data: '' }
		]);
		assertEqual(blob.type, 'application/zip', 'blob media type');
		assertEqual(blob.size, bytes.length, 'blob size equals the byte length');
		assertDeepEqual(
			Array.from(bytes.subarray(0, 4)),
			[0x50, 0x4b, 0x03, 0x04],
			'starts with PK\\x03\\x04'
		);
		assertEqual(parsed.firstSignature, 0x04034b50, 'first signature as a number');
		assertEqual(parsed.endEntriesTotal, 3, 'end record: total entries');
		assertEqual(parsed.endEntriesOnDisk, 3, 'end record: entries on this disk');
		assertEqual(parsed.entries.length, 3, 'one central record per file');
		assertDeepEqual(
			parsed.centralSignatures,
			[0x02014b50, 0x02014b50, 0x02014b50],
			'every central record has the PK\\x01\\x02 signature'
		);
		assertEqual(parsed.commentLength, 0, 'no archive comment');
	}
);

await suite.checkAsync(
	'createStoredZipBlob stores names, sizes, CRC-32 and data of every file',
	async () => {
		const { parsed } = await build([
			{ path: 'a.txt', data: '123456789' },
			{ path: 'dir/b.bin', data: new Uint8Array([1, 2, 3]) },
			{ path: 'empty.txt', data: '' }
		]);
		const [a, b, empty] = parsed.entries;

		assertEqual(a.name, 'a.txt', 'name a');
		assertEqual(a.size, 9, 'size a');
		assertEqual(a.compressedSize, 9, 'compressed size a equals size (stored)');
		assertEqual(a.crc, 0xcbf43926, 'crc a is the published check value');
		assertEqual(decoder.decode(a.data), '123456789', 'data a');

		assertEqual(b.name, 'dir/b.bin', 'name b keeps the folder');
		assertEqual(b.size, 3, 'size b');
		assertEqual(b.crc, 0x55bc801d, 'crc b (bytes 1 2 3)');
		assertDeepEqual(Array.from(b.data), [1, 2, 3], 'data b');

		assertEqual(empty.name, 'empty.txt', 'name empty');
		assertEqual(empty.size, 0, 'size empty');
		assertEqual(empty.crc, 0, 'crc empty');
		assertEqual(empty.data.length, 0, 'data empty');

		for (const entry of parsed.entries) {
			assertEqual(entry.method, 0, `${entry.name}: method 0 (stored)`);
			assertEqual(entry.flags, 0, `${entry.name}: no general-purpose flags`);
			assertEqual(entry.localHeaderCrc, entry.crc, `${entry.name}: local crc equals central crc`);
			assertEqual(
				entry.localHeaderSize,
				entry.size,
				`${entry.name}: local size equals central size`
			);
			assertEqual(
				entry.localHeaderName,
				entry.name,
				`${entry.name}: local name equals central name`
			);
		}
	}
);

await suite.checkAsync(
	'createStoredZipBlob lays out local entries back to back from offset 0',
	async () => {
		const { parsed } = await build([
			{ path: 'a.txt', data: '123456789' },
			{ path: 'dir/b.bin', data: new Uint8Array([1, 2, 3]) },
			{ path: 'empty.txt', data: '' }
		]);
		const [a, b, empty] = parsed.entries;
		assertEqual(a.localOffset, 0, 'first entry at offset 0');
		assertEqual(b.localOffset, 30 + 5 + 9, 'second entry after 30 + name 5 + data 9');
		assertEqual(empty.localOffset, 44 + 30 + 9 + 3, 'third entry after 30 + name 9 + data 3');
		assertEqual(parsed.centralOffset, 86 + 30 + 9 + 0, 'central directory after all local entries');
		assertEqual(
			parsed.centralSize,
			46 * 3 + 5 + 9 + 9,
			'central directory is 46 bytes per record plus the names'
		);
		assertEqual(parsed.length, parsed.centralOffset + parsed.centralSize + 22, 'total length');
	}
);

await suite.checkAsync(
	'createStoredZipBlob encodes file names and text as UTF-8 byte-exactly',
	async () => {
		const name = '报告/résumé-é.txt';
		const text = '你好, мир';
		const { bytes, parsed } = await build([{ path: name, data: text }]);
		const [entry] = parsed.entries;
		assertEqual(entry.name, name, 'name decodes back as UTF-8');
		assertEqual(entry.size, encoder.encode(text).length, 'size counts UTF-8 bytes, not characters');
		assert(entry.size > text.length, 'multi-byte text is longer in bytes');
		assertEqual(decoder.decode(entry.data), text, 'text decodes back');
		// CURRENT BEHAVIOUR: names are written as UTF-8 but general-purpose flag bit 11 (the
		// zip "language encoding" flag) is not set, so a strict reader decodes non-ASCII names as
		// CP437 and shows them garbled. Setting 0x0800 in both headers would fix it.
		assertEqual(entry.flags & 0x0800, 0, 'the UTF-8 name flag is not set');
		// The name bytes sit right after the 30-byte local header.
		assertDeepEqual(
			Array.from(bytes.subarray(30, 30 + encoder.encode(name).length)),
			Array.from(encoder.encode(name)),
			'name bytes in the local header'
		);
	}
);

await suite.checkAsync(
	'createStoredZipBlob with no files is a bare end-of-central-directory record',
	async () => {
		const { bytes, parsed } = await build([]);
		assertEqual(bytes.length, 22, 'only the end record');
		assertEqual(parsed.endEntriesTotal, 0, 'zero entries');
		assertEqual(parsed.centralOffset, 0, 'central directory offset 0');
		assertEqual(parsed.centralSize, 0, 'central directory size 0');
		assertDeepEqual(
			Array.from(bytes.subarray(0, 4)),
			[0x50, 0x4b, 0x05, 0x06],
			'starts with PK\\x05\\x06'
		);
	}
);

await suite.checkAsync(
	'createStoredZipBlob copies binary data exactly, including a subarray view',
	async () => {
		const backing = new Uint8Array(32).map((_, index) => index * 7);
		const view = backing.subarray(5, 21);
		const { parsed } = await build([{ path: 'view.bin', data: view }]);
		const [entry] = parsed.entries;
		assertEqual(entry.size, 16, 'size of the view, not of its buffer');
		assertDeepEqual(Array.from(entry.data), Array.from(view), 'bytes of the view');
		assertEqual(entry.crc, zip.crc32(view), 'crc of the view');
	}
);

await suite.checkAsync(
	'createStoredZipBlob keeps duplicate paths as separate entries',
	async () => {
		const { parsed } = await build([
			{ path: 'same.txt', data: 'one' },
			{ path: 'same.txt', data: 'two' }
		]);
		assertEqual(parsed.entries.length, 2, 'both entries written');
		assertEqual(decoder.decode(parsed.entries[0].data), 'one', 'first');
		assertEqual(decoder.decode(parsed.entries[1].data), 'two', 'second');
	}
);

await suite.checkAsync(
	'createStoredZipBlob stores a file of 70000 bytes with the exact size and CRC',
	async () => {
		const data = new Uint8Array(70_000).fill(0x61);
		const { parsed } = await build([{ path: 'big.bin', data }]);
		assertEqual(parsed.entries[0].size, 70_000, 'size above 16 bits');
		assertEqual(parsed.entries[0].data.length, 70_000, 'data length');
		assertEqual(parsed.entries[0].crc, zip.crc32(data), 'crc agrees with crc32');
	}
);

suite.check('zipBlobPart returns an exact copy in its own buffer', () => {
	const backing = new Uint8Array([9, 8, 7, 6, 5, 4]);
	const view = backing.subarray(1, 4);
	const part = zip.zipBlobPart(view);
	assert(part instanceof ArrayBuffer, 'an ArrayBuffer');
	assertEqual(part.byteLength, 3, 'only the viewed bytes');
	assertDeepEqual(Array.from(new Uint8Array(part)), [8, 7, 6], 'same bytes');
	new Uint8Array(part)[0] = 0;
	assertEqual(backing[1], 8, 'writing the copy does not change the source');
	assertEqual(zip.zipBlobPart(new Uint8Array(0)).byteLength, 0, 'empty input');
});

suite.check('safeExportSlug keeps letters, digits, dot, underscore and dash', () => {
	assertEqual(zip.safeExportSlug('Q4-report_v1.2'), 'Q4-report_v1.2', 'already safe');
	assertEqual(
		zip.safeExportSlug('  Q4 Report / 2026!  '),
		'Q4-Report-2026',
		'spaces and punctuation'
	);
	assertEqual(zip.safeExportSlug('a   b'), 'a-b', 'runs collapse to one dash');
	assertEqual(zip.safeExportSlug('--a--'), 'a', 'edge dashes are trimmed');
	assertEqual(zip.safeExportSlug('__a__'), '__a__', 'underscores are kept at the edges');
	assertEqual(zip.safeExportSlug(''), 'group', 'empty falls back to group');
	assertEqual(zip.safeExportSlug('   '), 'group', 'blank falls back to group');
	assertEqual(zip.safeExportSlug('中文'), 'group', 'non-ASCII only falls back to group');
	assertEqual(zip.safeExportSlug('名称 x'), 'x', 'non-ASCII removed, ASCII kept');
	assertEqual(zip.safeExportSlug('!!!'), 'group', 'punctuation only');
});

suite.check('safeZipSegment turns separators into dashes and never returns an empty name', () => {
	assertEqual(zip.safeZipSegment('photo.png'), 'photo.png', 'already safe');
	assertEqual(zip.safeZipSegment('a/b\\c d.txt'), 'a-b-c-d.txt', 'slashes and spaces');
	assertEqual(zip.safeZipSegment('/etc/passwd'), 'etc-passwd', 'leading slash');
	assertEqual(zip.safeZipSegment('  report (1).pdf '), 'report-1-.pdf', 'brackets');
	assertEqual(zip.safeZipSegment(''), 'attachment.bin', 'empty');
	assertEqual(zip.safeZipSegment('   '), 'attachment.bin', 'blank');
	assertEqual(zip.safeZipSegment('///'), 'attachment.bin', 'only separators');
	assertEqual(zip.safeZipSegment('文件'), 'attachment.bin', 'non-ASCII only');
	assertEqual(zip.safeZipSegment('文件.pdf'), '.pdf', 'non-ASCII removed, extension kept');
});

suite.check('safeZipSegment keeps a name made only of dots', () => {
	// CURRENT BEHAVIOUR: '.' and '..' pass through unchanged because dots are allowed. Used as a
	// zip path segment, '..' is the classic extraction-path-traversal name. Slashes cannot
	// survive, so the risk is limited to a segment that is exactly '..'.
	assertEqual(zip.safeZipSegment('..'), '..', 'two dots');
	assertEqual(zip.safeZipSegment('.'), '.', 'one dot');
	assertEqual(zip.safeZipSegment('../x'), '..-x', 'traversal prefix is neutralised');
});

suite.check('attachmentManifestUrls collects trimmed URLs from strings, arrays and objects', () => {
	assertDeepEqual(zip.attachmentManifestUrls('  https://a/x.png  '), ['https://a/x.png'], 'string');
	assertDeepEqual(zip.attachmentManifestUrls(''), [], 'empty string');
	assertDeepEqual(zip.attachmentManifestUrls('   '), [], 'blank string');
	assertDeepEqual(zip.attachmentManifestUrls({ url: 'https://a/1' }), ['https://a/1'], 'url');
	assertDeepEqual(zip.attachmentManifestUrls({ href: 'https://a/2' }), ['https://a/2'], 'href');
	assertDeepEqual(
		zip.attachmentManifestUrls({ url: 'https://a/1', href: 'https://a/2' }),
		['https://a/1'],
		'url wins over href'
	);
	assertDeepEqual(
		zip.attachmentManifestUrls(['u1', { url: 'u2' }, ['u3', { href: 'u4' }], '', null, 5]),
		['u1', 'u2', 'u3', 'u4'],
		'nested array, skipping empty and non-URL items'
	);
	assertDeepEqual(zip.attachmentManifestUrls({ src: 'https://a/3' }), [], 'src is not read');
	assertDeepEqual(zip.attachmentManifestUrls({}), [], 'empty object');
	assertDeepEqual(zip.attachmentManifestUrls(null), [], 'null');
	assertDeepEqual(zip.attachmentManifestUrls(undefined), [], 'undefined');
	assertDeepEqual(zip.attachmentManifestUrls(42), [], 'number');
	assertDeepEqual(zip.attachmentManifestUrls(true), [], 'boolean');
	assertDeepEqual(zip.attachmentManifestUrls({ url: 7 }), [], 'url that is not a string');
	assertDeepEqual(
		zip.attachmentManifestUrls({ url: 7, href: 'h' }),
		['h'],
		'non-string url, href used'
	);
});

suite.check('attachmentManifestUrls does not fall back to href when url is an empty string', () => {
	// CURRENT BEHAVIOUR: `{ url: '', href: 'https://a/2' }` returns [] because the string `url`
	// wins even when it is blank; a reader would expect the usable href.
	assertDeepEqual(zip.attachmentManifestUrls({ url: '', href: 'https://a/2' }), [], 'blank url');
});

finish(suite.result());
