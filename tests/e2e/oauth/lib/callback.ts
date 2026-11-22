// The client's side of the redirect: the loopback listener and the checks on what arrives.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { sleep } from './util.ts';

export const CALLBACK_PATH = '/oauth/sylvode/callback';

export type CallbackVerdict =
	| { ok: true; code: string }
	| { ok: false; kind: 'error'; error: string; description: string | undefined }
	| {
			ok: false;
			kind: 'invalid';
			reason:
				| 'wrong_path'
				| 'iss_missing'
				| 'iss_mismatch'
				| 'state_missing'
				| 'state_mismatch'
				| 'duplicate_parameter'
				| 'neither_code_nor_error';
	  };

/**
 * Checks a redirect the way contract 6.1 step 9 says a client must: the exact path first, then
 * `iss` (an absent one is rejected, because the server advertises that it sends it) and `state`,
 * and only then the code or the error text.
 *
 * `target` is the request target the listener saw (`/path?query`) or a full URL.
 */
export function parseCallback(
	target: string,
	expected: { path: string; iss: string; state: string }
): CallbackVerdict {
	let url: URL;
	try {
		url = new URL(target, 'http://127.0.0.1');
	} catch {
		return { ok: false, kind: 'invalid', reason: 'wrong_path' };
	}
	if (url.pathname !== expected.path) return { ok: false, kind: 'invalid', reason: 'wrong_path' };
	const counts = new Map<string, number>();
	for (const key of url.searchParams.keys()) counts.set(key, (counts.get(key) ?? 0) + 1);
	if ([...counts.values()].some((count) => count > 1)) {
		return { ok: false, kind: 'invalid', reason: 'duplicate_parameter' };
	}
	const iss = url.searchParams.get('iss');
	if (iss === null) return { ok: false, kind: 'invalid', reason: 'iss_missing' };
	if (iss !== expected.iss) return { ok: false, kind: 'invalid', reason: 'iss_mismatch' };
	const state = url.searchParams.get('state');
	if (state === null) return { ok: false, kind: 'invalid', reason: 'state_missing' };
	if (state !== expected.state) return { ok: false, kind: 'invalid', reason: 'state_mismatch' };
	const error = url.searchParams.get('error');
	if (error !== null) {
		return {
			ok: false,
			kind: 'error',
			error,
			description: url.searchParams.get('error_description') ?? undefined
		};
	}
	const code = url.searchParams.get('code');
	if (code === null || code === '')
		return { ok: false, kind: 'invalid', reason: 'neither_code_nor_error' };
	return { ok: true, code };
}

const PAGE =
	'<!doctype html><html><head><meta charset="utf-8"><title>Return to the application</title></head>' +
	'<body><p>You can return to the application.</p></body></html>';

/**
 * A one-shot loopback HTTP listener on 127.0.0.1 with an operating-system chosen port. It accepts
 * exactly one request on the callback path, answers a small page without script, and stops
 * listening. Requests to other paths get 404 and are listed in `strays`.
 */
export class CallbackListener {
	readonly port: number;
	readonly redirectUri: string;
	/** Request targets (path and query) of every request that reached the callback path. */
	readonly received: string[] = [];
	readonly strays: string[] = [];
	private readonly server: Server;
	private waiters: Array<(target: string) => void> = [];
	private closed = false;

	private constructor(server: Server, port: number) {
		this.server = server;
		this.port = port;
		this.redirectUri = `http://127.0.0.1:${port}${CALLBACK_PATH}`;
	}

	static async start(): Promise<CallbackListener> {
		let instance: CallbackListener | null = null;
		const server = createServer((req, res) => instance?.handle(req, res));
		await new Promise<void>((resolve, reject) => {
			server.once('error', reject);
			server.listen(0, '127.0.0.1', () => resolve());
		});
		const address = server.address() as AddressInfo;
		instance = new CallbackListener(server, address.port);
		return instance;
	}

	private handle(req: IncomingMessage, res: ServerResponse): void {
		const target = req.url ?? '/';
		const path = target.split('?')[0];
		if (path !== CALLBACK_PATH || this.received.length > 0) {
			if (path === CALLBACK_PATH) this.received.push(target);
			else this.strays.push(path ?? '');
			res.writeHead(path === CALLBACK_PATH ? 409 : 404, { 'Cache-Control': 'no-store' });
			res.end();
			return;
		}
		this.received.push(target);
		res.writeHead(200, {
			'Content-Type': 'text/html; charset=utf-8',
			'Cache-Control': 'no-store',
			Connection: 'close'
		});
		res.end(PAGE, () => {
			for (const waiter of this.waiters) waiter(target);
			this.waiters = [];
			void this.close();
		});
	}

	/** Resolves with the request target of the one callback, or rejects after `timeoutMs`. */
	waitForCallback(timeoutMs: number): Promise<string> {
		const first = this.received[0];
		if (first !== undefined) return Promise.resolve(first);
		return new Promise<string>((resolve, reject) => {
			const timer = setTimeout(
				() => reject(new Error(`no redirect reached the loopback listener within ${timeoutMs} ms`)),
				timeoutMs
			);
			this.waiters.push((target) => {
				clearTimeout(timer);
				resolve(target);
			});
		});
	}

	/** Resolves true when nothing reached the callback path within `ms`. */
	async stayedQuiet(ms: number): Promise<boolean> {
		await sleep(ms);
		return this.received.length === 0;
	}

	async close(): Promise<void> {
		if (this.closed) return;
		this.closed = true;
		this.server.closeAllConnections();
		await new Promise<void>((resolve) => this.server.close(() => resolve()));
	}
}
