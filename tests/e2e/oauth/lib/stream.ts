// The event stream as a client reads it: one outbound GET that never ends, every message stamped
// with a monotonic clock the moment the chunk that carried it arrived.

import { evidenceHeaders } from './http.ts';
import { redactor, truncate } from './redact.ts';
import type { Recorder } from './record.ts';
import { SseParser, type SseItem } from './sse.ts';
import { asRecord, mono, sleep, str } from './util.ts';

export interface Timed {
	item: SseItem;
	/** performance.now() when the carrying chunk arrived. */
	t: number;
}

export interface SvEvent {
	frameId: string | undefined;
	type: string;
	workspaceId: string | undefined;
	projectId: string | undefined;
	subject: Record<string, unknown>;
	envelope: Record<string, unknown>;
	t: number;
}

export interface Control {
	type: string;
	reason: string | undefined;
	body: Record<string, unknown>;
	t: number;
}

function parseData(item: SseItem): Record<string, unknown> | null {
	if (item.kind !== 'message') return null;
	try {
		return asRecord(JSON.parse(item.data) as unknown);
	} catch {
		return null;
	}
}

export function asEvent(timed: Timed): SvEvent | null {
	if (timed.item.kind !== 'message' || timed.item.event !== 'sv.event') return null;
	const envelope = parseData(timed.item);
	if (envelope === null) return null;
	return {
		frameId: timed.item.id,
		type: str(envelope.type) ?? '',
		workspaceId: str(envelope.workspace_id),
		projectId: str(asRecord(envelope.project)?.id),
		subject: asRecord(envelope.subject) ?? {},
		envelope,
		t: timed.t
	};
}

export function asControl(timed: Timed): Control | null {
	if (timed.item.kind !== 'message' || timed.item.event !== 'sv.control') return null;
	const body = parseData(timed.item);
	if (body === null) return null;
	return { type: str(body.type) ?? '', reason: str(body.reason), body, t: timed.t };
}

export interface OpenResult {
	status: number;
	headers: Headers;
	/** The body of a refusal; empty when the stream opened. */
	refusalText: string;
	stream: EventStream | null;
}

export class EventStream {
	readonly items: Timed[] = [];
	readonly openedAt: number;
	ended = false;
	endedAt: number | undefined;
	endReason: string | undefined;
	private readonly parser = new SseParser();
	private readonly abort = new AbortController();
	private waiters: Array<() => void> = [];
	private done: Promise<void> = Promise.resolve();
	readonly label: string;

	private constructor(label: string) {
		this.label = label;
		this.openedAt = mono();
	}

	/** Opens the stream. A refusal (401, 429, ...) comes back as a status with no stream. */
	static async open(args: {
		rec: Recorder;
		label: string;
		url: string;
		token: string;
		lastEventId?: string;
		types?: string;
	}): Promise<OpenResult> {
		const url = new URL(args.url);
		if (args.types !== undefined) url.searchParams.set('types', args.types);
		const headers: Record<string, string> = {
			accept: 'text/event-stream',
			authorization: `Bearer ${args.token}`
		};
		if (args.lastEventId !== undefined) headers['last-event-id'] = args.lastEventId;
		const stream = new EventStream(args.label);
		const started = mono();
		let response: Response;
		try {
			response = await fetch(url, { headers, signal: stream.abort.signal, redirect: 'manual' });
		} catch (error) {
			const message = redactor.text(error instanceof Error ? error.message : String(error));
			args.rec.add({
				label: args.label,
				method: 'GET',
				url: redactor.url(url.toString()),
				requestBody: undefined,
				status: 0,
				headers: {},
				body: `transport error: ${message}`,
				elapsedMs: Math.round(mono() - started)
			});
			throw new Error(`${args.label}: transport error: ${message}`);
		}
		const contentType = response.headers.get('content-type') ?? '';
		const isStream =
			response.status === 200 &&
			contentType.includes('text/event-stream') &&
			response.body !== null;
		if (!isStream) {
			const text = await response.text();
			args.rec.add({
				label: args.label,
				method: 'GET',
				url: redactor.url(url.toString()),
				requestBody: undefined,
				status: response.status,
				headers: evidenceHeaders(response.headers),
				body: truncate(redactor.body(text, contentType), 2000),
				elapsedMs: Math.round(mono() - started)
			});
			return {
				status: response.status,
				headers: response.headers,
				refusalText: text,
				stream: null
			};
		}
		args.rec.add({
			label: args.label,
			method: 'GET',
			url: redactor.url(url.toString()),
			requestBody: undefined,
			status: response.status,
			headers: evidenceHeaders(response.headers),
			body: '(event stream; messages are listed in the case notes)',
			elapsedMs: Math.round(mono() - started)
		});
		stream.done = stream.read(response.body as ReadableStream<Uint8Array>);
		return { status: 200, headers: response.headers, refusalText: '', stream };
	}

	private wake(): void {
		const waiting = this.waiters;
		this.waiters = [];
		for (const resume of waiting) resume();
	}

	private async read(body: ReadableStream<Uint8Array>): Promise<void> {
		const decoder = new TextDecoder();
		const reader = body.getReader();
		try {
			for (;;) {
				const { done, value } = await reader.read();
				const t = mono();
				if (done) {
					this.endReason = 'closed by the server';
					break;
				}
				for (const item of this.parser.push(decoder.decode(value, { stream: true })))
					this.items.push({ item, t });
				this.wake();
			}
		} catch (error) {
			this.endReason = this.abort.signal.aborted
				? 'closed by the client'
				: redactor.text(`read error: ${error instanceof Error ? error.message : String(error)}`);
		} finally {
			this.ended = true;
			this.endedAt = mono();
			this.wake();
		}
	}

	/** The first item at or after index `from` that satisfies `predicate`, waiting up to `timeoutMs`. */
	async waitFor(
		predicate: (timed: Timed) => boolean,
		timeoutMs: number,
		from = 0
	): Promise<Timed | null> {
		const deadline = mono() + timeoutMs;
		let index = from;
		for (;;) {
			while (index < this.items.length) {
				const timed = this.items[index];
				index += 1;
				if (timed !== undefined && predicate(timed)) return timed;
			}
			if (this.ended) return null;
			const remaining = deadline - mono();
			if (remaining <= 0) return null;
			await new Promise<void>((resolve) => {
				const timer = setTimeout(resolve, remaining);
				this.waiters.push(() => {
					clearTimeout(timer);
					resolve();
				});
			});
		}
	}

	waitForControl(type: string, timeoutMs: number, from = 0): Promise<Timed | null> {
		return this.waitFor((timed) => asControl(timed)?.type === type, timeoutMs, from);
	}

	waitForEvent(
		match: (event: SvEvent) => boolean,
		timeoutMs: number,
		from = 0
	): Promise<Timed | null> {
		return this.waitFor(
			(timed) => {
				const event = asEvent(timed);
				return event !== null && match(event);
			},
			timeoutMs,
			from
		);
	}

	events(): SvEvent[] {
		return this.items.map(asEvent).filter((event): event is SvEvent => event !== null);
	}

	controls(): Control[] {
		return this.items.map(asControl).filter((control): control is Control => control !== null);
	}

	hello(): Control | undefined {
		return this.controls().find((control) => control.type === 'hello');
	}

	/** Resolves once the stream has ended (closed by either side). */
	async untilEnded(timeoutMs: number): Promise<boolean> {
		const deadline = mono() + timeoutMs;
		while (!this.ended && mono() < deadline) await sleep(25);
		return this.ended;
	}

	/** The position a reconnecting client would send: the last frame id, else the hello cursor. */
	resumeId(): string | undefined {
		return this.parser.lastEventId !== ''
			? this.parser.lastEventId
			: str(this.hello()?.body.cursor);
	}

	async close(): Promise<void> {
		this.abort.abort();
		await this.done;
	}

	/** One line per message, redacted, for the case notes. */
	transcript(): string[] {
		return this.items
			.map(({ item, t }) => {
				const at = `+${Math.round(t - this.openedAt)}ms`;
				if (item.kind === 'comment') return `${at} comment: ${redactor.text(item.text)}`;
				if (item.kind === 'retry') return `${at} retry: ${item.ms}`;
				return `${at} ${item.event}${item.id === undefined ? '' : ` id=${redactor.text(item.id)}`}: ${truncate(redactor.body(item.data, 'application/json'), 600)}`;
			})
			.concat(this.ended ? [`end: ${this.endReason ?? 'ended'}`] : []);
	}
}
