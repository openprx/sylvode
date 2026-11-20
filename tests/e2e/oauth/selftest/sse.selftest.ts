import assert from 'node:assert/strict';
import { test } from 'node:test';
import { SseParser, type SseItem } from '../lib/sse.ts';

const parseAll = (chunks: string[]): SseItem[] => {
	const parser = new SseParser();
	return chunks.flatMap((chunk) => parser.push(chunk));
};

const STREAM =
	'event: sv.control\ndata: {"type":"hello"}\n\n' +
	': keep-alive\n\n' +
	'id: 7\nevent: sv.event\ndata: line one\ndata: line two\n\n' +
	'retry: 3000\n\n' +
	'data: no event name\n\n';

const EXPECTED: SseItem[] = [
	{ kind: 'message', event: 'sv.control', data: '{"type":"hello"}', id: undefined },
	{ kind: 'comment', text: 'keep-alive' },
	{ kind: 'message', event: 'sv.event', data: 'line one\nline two', id: '7' },
	{ kind: 'retry', ms: 3000 },
	{ kind: 'message', event: 'message', data: 'no event name', id: undefined }
];

test('a whole stream parses into messages, comments and retry', () => {
	assert.deepEqual(parseAll([STREAM]), EXPECTED);
});

test('splitting the stream at every byte boundary gives the same result', () => {
	for (let cut = 1; cut < STREAM.length; cut += 1) {
		assert.deepEqual(
			parseAll([STREAM.slice(0, cut), STREAM.slice(cut)]),
			EXPECTED,
			`cut at ${cut}`
		);
	}
	assert.deepEqual(parseAll([...STREAM].map((char) => char)), EXPECTED);
});

test('CRLF, lone CR and a CRLF split between chunks are line ends', () => {
	const crlf = STREAM.replace(/\n/g, '\r\n');
	assert.deepEqual(parseAll([crlf]), EXPECTED);
	// A final lone CR could be the first half of a CRLF, so it waits for the next byte.
	assert.deepEqual(parseAll([STREAM.replace(/\n/g, '\r'), ': tail']), EXPECTED);
	for (let cut = 1; cut < crlf.length; cut += 1)
		assert.deepEqual(
			parseAll([crlf.slice(0, cut), crlf.slice(cut)]),
			EXPECTED,
			`crlf cut at ${cut}`
		);
});

test('a frame is not delivered before its blank line, and one without data is not delivered', () => {
	const parser = new SseParser();
	assert.deepEqual(parser.push('event: x\ndata: a\n'), []);
	assert.deepEqual(parser.push('\n'), [{ kind: 'message', event: 'x', data: 'a', id: undefined }]);
	assert.deepEqual(parser.push('event: only-a-name\n\n'), []);
});

test('id is remembered across frames, a NUL id is ignored, a leading BOM is dropped, a field without colon works', () => {
	const parser = new SseParser();
	parser.push('﻿id: 1\ndata: a\n\ndata: b\n\n');
	assert.equal(parser.lastEventId, '1');
	parser.push('id: bad\u0000id\ndata: c\n\n');
	assert.equal(parser.lastEventId, '1');
	assert.deepEqual(parser.push('data\n\n'), [
		{ kind: 'message', event: 'message', data: '', id: undefined }
	]);
});

test('a retry that is not a number is ignored and only one leading space is stripped', () => {
	assert.deepEqual(parseAll(['retry: soon\n\ndata:  two spaces\n\n']), [
		{ kind: 'message', event: 'message', data: ' two spaces', id: undefined }
	]);
});
