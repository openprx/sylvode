// Server-sent events framing (WHATWG HTML, "Parsing an event stream"), as a push parser.
//
// Handles frames split across chunks at any byte boundary, CRLF, LF and lone CR line ends (a CR
// at the end of a chunk waits for a possible LF in the next one), comment lines, multi-line data,
// `id:` and `retry:` fields, and the optional leading byte order mark.

export type SseItem =
	| { kind: 'message'; event: string; data: string; id: string | undefined }
	| { kind: 'comment'; text: string }
	| { kind: 'retry'; ms: number };

export class SseParser {
	private buffer = '';
	private started = false;
	private dataLines: string[] = [];
	private eventName = '';
	private frameId: string | undefined;
	/** The last `id:` value seen on any frame (the resume position a client would send back). */
	lastEventId = '';

	push(chunk: string): SseItem[] {
		let text = chunk;
		if (!this.started) {
			this.started = true;
			if (text.startsWith('﻿')) text = text.slice(1);
		}
		this.buffer += text;
		const items: SseItem[] = [];
		for (;;) {
			const match = /\r\n|\n|\r/.exec(this.buffer);
			if (match === null) break;
			// A CR at the very end may be the first half of a CRLF: wait for the next chunk.
			if (match[0] === '\r' && match.index === this.buffer.length - 1) break;
			const line = this.buffer.slice(0, match.index);
			this.buffer = this.buffer.slice(match.index + match[0].length);
			this.line(line, items);
		}
		return items;
	}

	private line(line: string, items: SseItem[]): void {
		if (line === '') {
			if (this.dataLines.length > 0) {
				items.push({
					kind: 'message',
					event: this.eventName === '' ? 'message' : this.eventName,
					data: this.dataLines.join('\n'),
					id: this.frameId
				});
			}
			this.dataLines = [];
			this.eventName = '';
			this.frameId = undefined;
			return;
		}
		if (line.startsWith(':')) {
			items.push({ kind: 'comment', text: line.slice(1).replace(/^ /, '') });
			return;
		}
		const colon = line.indexOf(':');
		const field = colon === -1 ? line : line.slice(0, colon);
		let value = colon === -1 ? '' : line.slice(colon + 1);
		if (value.startsWith(' ')) value = value.slice(1);
		switch (field) {
			case 'event':
				this.eventName = value;
				break;
			case 'data':
				this.dataLines.push(value);
				break;
			case 'id':
				if (!value.includes('\0')) {
					this.frameId = value;
					this.lastEventId = value;
				}
				break;
			case 'retry':
				if (/^[0-9]+$/.test(value)) items.push({ kind: 'retry', ms: Number(value) });
				break;
			default:
				break;
		}
	}
}
