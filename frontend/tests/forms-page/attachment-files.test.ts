/**
 * Attachment names, content types, URLs and signature value helpers of the forms page.
 *
 * The module has no size limit: `max_size_mb` of a field is read elsewhere. The limits it does
 * implement are the accept list of a field, the image extensions it recognises, and the
 * allowed prefixes of a signature value; those are tested at their boundaries below.
 *
 * Run standalone: `bun tests/forms-page/attachment-files.test.ts`
 */
import { Suite, assert, assertEqual, finish } from '../support/harness';
import { installSvelteSsr } from '../support/svelte-ssr';
import type { FormAttachment, FormField, FormRecord } from '../../src/lib/api/forms';

await installSvelteSsr();
const files = await import('../../src/lib/forms/attachment-files');

const suite = new Suite('forms-page/attachment-files');

function field(type: FormField['type'] | undefined, extra: Partial<FormField> = {}): FormField {
	return { key: 'doc', type, ...extra };
}

function recordWith(values: Record<string, unknown>): FormRecord {
	return {
		id: 'r',
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		title: 'R',
		values,
		source: {},
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function attachment(overrides: Partial<FormAttachment> = {}): FormAttachment {
	return {
		id: 'a',
		workspace_id: 'w',
		project_id: 'p',
		form_id: 'f',
		field_id: 'fld',
		field_key: 'doc',
		file_name: 'file.bin',
		content_type: 'application/octet-stream',
		byte_size: 10,
		storage_key: 'k',
		url: 'https://files.example/a/file.bin',
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		...overrides
	};
}

suite.check('recordMediaUrl reads a string, an object or the first usable array item', () => {
	const f = field('image');
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: 'https://a/x.png' }), f),
		'https://a/x.png',
		'string'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: { url: 'u', href: 'h', src: 's' } }), f),
		'u',
		'url first'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: { href: 'h', src: 's' } }), f),
		'h',
		'then href'
	);
	assertEqual(files.recordMediaUrl(recordWith({ doc: { src: 's' } }), f), 's', 'then src');
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: { name: 'n' } }), f),
		'',
		'object without a URL'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: ['a1', 'a2'] }), f),
		'a1',
		'first string of an array'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: [1, null, { url: 'u1' }, 'a2'] }), f),
		'u1',
		'skips non-URL items'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: [{ href: 'h1' }] }), f),
		'h1',
		'array item href'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: [{ src: 's1' }] }), f),
		's1',
		'array item src'
	);
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: [{ name: 'n' }] }), f),
		'',
		'array item without a URL'
	);
	assertEqual(files.recordMediaUrl(recordWith({ doc: [] }), f), '', 'empty array');
	assertEqual(
		files.recordMediaUrl(recordWith({ doc: [1, 2] }), f),
		'',
		'array without usable items'
	);
	assertEqual(files.recordMediaUrl(recordWith({ doc: 5 }), f), '', 'number');
	assertEqual(files.recordMediaUrl(recordWith({ doc: null }), f), '', 'null');
	assertEqual(files.recordMediaUrl(recordWith({}), f), '', 'missing value');
	assertEqual(files.recordMediaUrl(recordWith({ doc: 'x' }), null), '', 'no field');
});

suite.check('attachmentFileName takes the last path segment of an absolute URL, decoded', () => {
	const f = field('attachment');
	assertEqual(
		files.attachmentFileName('https://files.example/a/report.pdf', f),
		'report.pdf',
		'plain'
	);
	assertEqual(
		files.attachmentFileName('https://files.example/a/b%20c%C3%A9.pdf', f),
		'b cé.pdf',
		'percent-decoded'
	);
	assertEqual(
		files.attachmentFileName('https://files.example/a/report.pdf?sig=1#top', f),
		'report.pdf',
		'query and fragment ignored'
	);
	assertEqual(files.attachmentFileName('https://files.example/a/dir/', f), 'dir', 'trailing slash');
	assertEqual(
		files.attachmentFileName('  https://files.example/x.txt  ', f),
		'x.txt',
		'URL is trimmed'
	);
});

suite.check('attachmentFileName takes the last segment of a relative path as it is', () => {
	const f = field('attachment');
	assertEqual(files.attachmentFileName('/uploads/a/b.png', f), 'b.png', 'rooted path');
	assertEqual(files.attachmentFileName('b.png', f), 'b.png', 'bare name');
	assertEqual(files.attachmentFileName('uploads//b.png', f), 'b.png', 'double slash');
	assertEqual(
		files.attachmentFileName('/uploads/b%20c.png', f),
		'b%20c.png',
		'a relative path is not decoded'
	);
	assertEqual(
		files.attachmentFileName('/uploads/b.png?v=2', f),
		'b.png?v=2',
		'a relative path keeps its query'
	);
});

suite.check(
	'attachmentFileName falls back to the field key for empty, root or undecodable input',
	() => {
		assertEqual(files.attachmentFileName('', field('attachment')), 'doc.bin', 'empty, attachment');
		assertEqual(
			files.attachmentFileName('   ', field('attachment')),
			'doc.bin',
			'blank, attachment'
		);
		assertEqual(files.attachmentFileName('', field('image')), 'doc.png', 'empty, image');
		assertEqual(files.attachmentFileName('', field(undefined)), 'doc.bin', 'empty, no type');
		assertEqual(
			files.attachmentFileName('https://files.example', field('attachment')),
			'doc.bin',
			'host only'
		);
		assertEqual(
			files.attachmentFileName('https://files.example/', field('image')),
			'doc.png',
			'host and slash, image'
		);
		assertEqual(files.attachmentFileName('/', field('attachment')), 'doc.bin', 'only a slash');
		assertEqual(files.attachmentFileName('///', field('image')), 'doc.png', 'only slashes');
		assertEqual(
			files.attachmentFileName('https://files.example/a/%E0%A4%A', field('attachment')),
			'%E0%A4%A',
			'a failed decode takes the raw last segment of the whole string'
		);
	}
);

suite.check('attachmentContentType maps image-field extensions and defaults to png', () => {
	const f = field('image');
	assertEqual(files.attachmentContentType('a.jpg', f), 'image/jpeg', 'jpg');
	assertEqual(files.attachmentContentType('a.JPEG', f), 'image/jpeg', 'JPEG upper case');
	assertEqual(files.attachmentContentType('a.webp', f), 'image/webp', 'webp');
	assertEqual(files.attachmentContentType('a.GIF', f), 'image/gif', 'GIF upper case');
	assertEqual(files.attachmentContentType('a.png', f), 'image/png', 'png');
	assertEqual(files.attachmentContentType('a.bmp', f), 'image/png', 'unlisted extension');
	assertEqual(files.attachmentContentType('a.svg', f), 'image/png', 'svg is not passed through');
	assertEqual(files.attachmentContentType('a.pdf', f), 'image/png', 'pdf in an image field');
	assertEqual(files.attachmentContentType('noext', f), 'image/png', 'no extension');
	assertEqual(files.attachmentContentType('', f), 'image/png', 'empty name');
});

suite.check('attachmentContentType maps file-field extensions and defaults to octet-stream', () => {
	for (const type of ['attachment', undefined, 'text'] as const) {
		const f = field(type);
		assertEqual(files.attachmentContentType('a.pdf', f), 'application/pdf', `${type}: pdf`);
		assertEqual(
			files.attachmentContentType('A.PDF', f),
			'application/pdf',
			`${type}: PDF upper case`
		);
		assertEqual(files.attachmentContentType('a.png', f), 'image/png', `${type}: png`);
		assertEqual(files.attachmentContentType('a.jpg', f), 'image/jpeg', `${type}: jpg`);
		assertEqual(files.attachmentContentType('a.jpeg', f), 'image/jpeg', `${type}: jpeg`);
		assertEqual(
			files.attachmentContentType('a.gif', f),
			'application/octet-stream',
			`${type}: gif is not mapped`
		);
		assertEqual(
			files.attachmentContentType('a.webp', f),
			'application/octet-stream',
			`${type}: webp is not mapped`
		);
		assertEqual(
			files.attachmentContentType('a.zip', f),
			'application/octet-stream',
			`${type}: zip`
		);
		assertEqual(
			files.attachmentContentType('pdf', f),
			'application/octet-stream',
			`${type}: name without a dot`
		);
		assertEqual(
			files.attachmentContentType('a.pdf.exe', f),
			'application/octet-stream',
			`${type}: the last extension decides`
		);
		assertEqual(
			files.attachmentContentType('', f),
			'application/octet-stream',
			`${type}: empty name`
		);
	}
});

suite.check('attachmentAcceptAttr joins the accept list, with an image default', () => {
	assertEqual(
		files.attachmentAcceptAttr(
			field('attachment', { attachment: { accept: ['.pdf', 'image/png'] } })
		),
		'.pdf,image/png',
		'two entries'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('attachment', { attachment: { accept: ['.pdf'] } })),
		'.pdf',
		'one entry'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('attachment', { attachment: { accept: ['', '.pdf', ''] } })),
		'.pdf',
		'empty entries dropped'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('attachment', { attachment: { accept: [] } })),
		undefined,
		'empty list, attachment'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('attachment', { attachment: { accept: [''] } })),
		undefined,
		'only empty entries'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('attachment', { attachment: {} })),
		undefined,
		'no accept, attachment'
	);
	assertEqual(files.attachmentAcceptAttr(field('attachment')), undefined, 'no attachment config');
	assertEqual(files.attachmentAcceptAttr(field('image')), 'image/*', 'image default');
	assertEqual(
		files.attachmentAcceptAttr(field('image', { attachment: { accept: [] } })),
		'image/*',
		'image with an empty list'
	);
	assertEqual(
		files.attachmentAcceptAttr(field('image', { attachment: { accept: ['image/png'] } })),
		'image/png',
		'image with its own list'
	);
});

suite.check(
	'attachmentStorageName replaces unsafe characters and never returns an empty name',
	() => {
		assertEqual(
			files.attachmentStorageName('report-v1.2_final.pdf'),
			'report-v1.2_final.pdf',
			'already safe'
		);
		assertEqual(
			files.attachmentStorageName('my report (1).pdf'),
			'my_report_1_.pdf',
			'spaces and brackets'
		);
		assertEqual(files.attachmentStorageName('a///b'), 'a_b', 'runs become one underscore');
		assertEqual(files.attachmentStorageName('  x  '), 'x', 'edge underscores trimmed');
		assertEqual(
			files.attachmentStorageName('__x__'),
			'x',
			'underscores at the edges are trimmed too'
		);
		assertEqual(files.attachmentStorageName(''), 'file', 'empty');
		assertEqual(files.attachmentStorageName('!!!'), 'file', 'only unsafe characters');
		assertEqual(
			files.attachmentStorageName('文件.pdf'),
			'.pdf',
			'non-ASCII stem becomes a leading underscore that is then trimmed'
		);
		assertEqual(files.attachmentStorageName('文件'), 'file', 'non-ASCII only');
		assertEqual(
			files.attachmentStorageName('../etc/passwd'),
			'.._etc_passwd',
			'path separators are removed'
		);
	}
);

suite.check('attachmentThumbnailUrl prefers the thumbnail and passes only safe image URLs', () => {
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: 'https://t.example/t.png' })),
		'https://t.example/t.png',
		'thumbnail'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: '' })),
		'https://files.example/a/file.bin',
		'empty thumbnail uses url'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: null })),
		'https://files.example/a/file.bin',
		'null thumbnail uses url'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: undefined })),
		'https://files.example/a/file.bin',
		'missing thumbnail uses url'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ url: '/api/v1/uploads/x.png' })),
		'/api/v1/uploads/x.png',
		'relative URL kept'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: 'javascript:alert(1)' })),
		'',
		'javascript: is dropped'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: 'data:image/png;base64,AAAA' })),
		'data:image/png;base64,AAAA',
		'raster data URL kept'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: 'data:image/svg+xml;base64,AAAA' })),
		'',
		'svg data URL dropped'
	);
	assertEqual(
		files.attachmentThumbnailUrl(attachment({ thumbnail_url: '', url: '' })),
		'',
		'nothing at all'
	);
});

suite.check(
	'attachmentIsPreviewableImage uses the content type first, then the file extension',
	() => {
		assert(
			files.attachmentIsPreviewableImage(attachment({ content_type: 'image/png', file_name: 'x' })),
			'image content type'
		);
		assert(
			files.attachmentIsPreviewableImage(
				attachment({ content_type: 'image/tiff', file_name: 'x.tif' })
			),
			'any image/* content type'
		);
		assert(
			files.attachmentIsPreviewableImage(
				attachment({ content_type: 'application/octet-stream', file_name: 'a.JPG' })
			),
			'jpg extension'
		);
		assert(
			files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.jpeg' })),
			'jpeg extension'
		);
		assert(
			files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.webp' })),
			'webp extension'
		);
		assert(
			files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.gif' })),
			'gif extension'
		);
		assert(
			files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.png' })),
			'png extension'
		);
		assert(
			!files.attachmentIsPreviewableImage(
				attachment({ content_type: 'application/pdf', file_name: 'a.pdf' })
			),
			'pdf'
		);
		assert(
			!files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.bmp' })),
			'bmp is not previewable by extension'
		);
		assert(
			!files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'a.png.txt' })),
			'the extension must be last'
		);
		assert(
			!files.attachmentIsPreviewableImage(attachment({ content_type: '', file_name: 'png' })),
			'no dot'
		);
		assert(
			!files.attachmentIsPreviewableImage(
				attachment({ content_type: 'IMAGE/PNG', file_name: 'x' })
			),
			'content type match is case-sensitive'
		);
		// The cast leaves out content_type on purpose (a malformed server payload).
		const noType = {
			...attachment({ file_name: 'a.png' }),
			content_type: undefined
		} as unknown as FormAttachment;
		assert(
			files.attachmentIsPreviewableImage(noType),
			'missing content type falls back to the extension'
		);
	}
);

suite.check('recordValueUrl reads a string or the url of an object', () => {
	assertEqual(files.recordValueUrl('https://a/x'), 'https://a/x', 'string');
	assertEqual(files.recordValueUrl(''), '', 'empty string');
	assertEqual(files.recordValueUrl({ url: 'https://a/y' }), 'https://a/y', 'object url');
	assertEqual(files.recordValueUrl({ href: 'https://a/z' }), '', 'href is not read');
	assertEqual(files.recordValueUrl({ url: 5 }), '', 'non-string url');
	assertEqual(files.recordValueUrl(['https://a']), '', 'array');
	assertEqual(files.recordValueUrl(null), '', 'null');
	assertEqual(files.recordValueUrl(undefined), '', 'undefined');
	assertEqual(files.recordValueUrl(7), '', 'number');
});

suite.check('signatureImageValue accepts only the three allowed prefixes', () => {
	const png = 'data:image/png;base64,AAAA';
	assertEqual(files.signatureImageValue({ sig: png }, 'sig'), png, 'data image');
	// CURRENT BEHAVIOUR: any data:image/ prefix passes, svg+xml included, although safeImageUrl
	// (used for thumbnails) rejects svg data URLs because svg can carry scripts.
	assertEqual(
		files.signatureImageValue({ sig: 'data:image/svg+xml;base64,AAAA' }, 'sig'),
		'data:image/svg+xml;base64,AAAA',
		'svg data URL is accepted'
	);
	assertEqual(
		files.signatureImageValue({ sig: '/api/v1/uploads/signatures/a.png' }, 'sig'),
		'/api/v1/uploads/signatures/a.png',
		'api upload path'
	);
	assertEqual(
		files.signatureImageValue({ sig: '/uploads/signatures/a.png' }, 'sig'),
		'/uploads/signatures/a.png',
		'upload path'
	);
	assertEqual(
		files.signatureImageValue({ sig: 'data:image' }, 'sig'),
		'',
		'prefix needs the slash'
	);
	assertEqual(
		files.signatureImageValue({ sig: 'data:text/html,<b>' }, 'sig'),
		'',
		'other data type'
	);
	assertEqual(
		files.signatureImageValue({ sig: ' data:image/png;base64,AAAA' }, 'sig'),
		'',
		'a leading space is rejected'
	);
	assertEqual(
		files.signatureImageValue({ sig: '/uploads/signatures' }, 'sig'),
		'',
		'folder without the trailing slash'
	);
	assertEqual(
		files.signatureImageValue({ sig: '/uploads/other/a.png' }, 'sig'),
		'',
		'other folder'
	);
	assertEqual(
		files.signatureImageValue({ sig: 'https://evil.example/s.png' }, 'sig'),
		'',
		'external URL'
	);
	assertEqual(files.signatureImageValue({ sig: 'javascript:alert(1)' }, 'sig'), '', 'javascript:');
	assertEqual(files.signatureImageValue({ sig: '' }, 'sig'), '', 'empty');
	assertEqual(files.signatureImageValue({ sig: 5 }, 'sig'), '', 'number');
	assertEqual(files.signatureImageValue({ sig: { url: png } }, 'sig'), '', 'object');
	assertEqual(files.signatureImageValue({}, 'sig'), '', 'missing key');
	assertEqual(files.signatureImageValue({ other: png }, 'sig'), '', 'other key');
});

suite.check('signaturePoint is the pointer position relative to the canvas corner', () => {
	// Minimal stand-ins are cast on purpose: the function reads only clientX/clientY and the
	// canvas bounding rectangle, and the test runs without a DOM.
	const canvas = {
		getBoundingClientRect: () => ({ left: 100, top: 50 })
	} as unknown as HTMLCanvasElement;
	const at = (clientX: number, clientY: number) =>
		({ clientX, clientY }) as unknown as PointerEvent;
	assertEqual(
		JSON.stringify(files.signaturePoint(at(130, 90), canvas)),
		'{"x":30,"y":40}',
		'inside'
	);
	assertEqual(JSON.stringify(files.signaturePoint(at(100, 50), canvas)), '{"x":0,"y":0}', 'corner');
	assertEqual(
		JSON.stringify(files.signaturePoint(at(90, 40), canvas)),
		'{"x":-10,"y":-10}',
		'outside gives negative values'
	);
	assertEqual(
		JSON.stringify(files.signaturePoint(at(100.5, 50.25), canvas)),
		'{"x":0.5,"y":0.25}',
		'fractions'
	);
});

finish(suite.result());
