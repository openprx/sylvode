// Checks that scripts/lib/e2e-web-proxy.ts passes a long-lived streaming response through: the
// first chunk reaches the client at once (no buffering), a quiet period longer than Bun's default
// idle timeout does not cut the connection, and a client that goes away closes the upstream
// request. Needs Bun only: the "API" is a stub started here. Takes about 14 seconds.
//
// Run: bun scripts/lib/e2e-web-proxy-streaming-check.ts

import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const QUIET_MS = 12_500; // longer than Bun's default 10 s idle timeout, shorter than the 20 s keep-alive
const STREAM = '/api/v1/oauth/events/stream';
const enc = new TextEncoder();
let upstreamClosed = false;
let upstreamHits = 0;

const stub = Bun.serve({
	hostname: '127.0.0.1',
	port: 0,
	idleTimeout: 0,
	fetch(req) {
		upstreamHits += 1;
		const body = new ReadableStream({
			async start(controller) {
				controller.enqueue(enc.encode('event: sv.control\ndata: {"type":"hello"}\n\n'));
				await Bun.sleep(QUIET_MS);
				try {
					controller.enqueue(enc.encode(': keep-alive\n\n'));
				} catch {
					// the client left during the quiet period
				}
			},
			cancel() {
				upstreamClosed = true;
			}
		});
		req.signal.addEventListener('abort', () => {
			upstreamClosed = true;
		});
		return new Response(body, {
			headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' }
		});
	}
});

const dir = mkdtempSync(join(tmpdir(), 'e2e-proxy-stream-'));
writeFileSync(join(dir, 'index.html'), '<!doctype html>\n');
const probe = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: () => new Response('') });
const proxyPort = probe.port;
probe.stop(true);
const proxy = Bun.spawn(['bun', join(import.meta.dir, 'e2e-web-proxy.ts')], {
	env: {
		...process.env,
		E2E_PROXY_BUILD_DIR: dir,
		E2E_PROXY_API_ORIGIN: `http://127.0.0.1:${stub.port}`,
		E2E_PROXY_PORT: String(proxyPort),
		E2E_PROXY_HOST: '127.0.0.1'
	},
	stdout: 'ignore',
	stderr: 'inherit'
});

let failures = 0;
const check = (ok: boolean, what: string) => {
	console.log(`${ok ? 'PASS' : 'FAIL'}: ${what}`);
	if (!ok) failures += 1;
};

try {
	for (let i = 0; i < 50; i += 1) {
		try {
			await fetch(`http://127.0.0.1:${proxyPort}/`);
			break;
		} catch {
			await Bun.sleep(100);
		}
	}
	const started = Date.now();
	const response = await fetch(`http://127.0.0.1:${proxyPort}${STREAM}`);
	check(
		response.headers.get('content-type') === 'text/event-stream',
		'the stream keeps its content type'
	);
	const reader = response.body!.getReader();
	const decoder = new TextDecoder();
	const first = await reader.read();
	const firstAt = Date.now() - started;
	check(
		!first.done && decoder.decode(first.value).includes('"type":"hello"') && firstAt < 2_000,
		`the first chunk arrives at once (${firstAt} ms), not when the response ends`
	);
	let second = '';
	let survived = true;
	try {
		const next = await reader.read();
		second = next.done ? '' : decoder.decode(next.value);
	} catch {
		survived = false;
	}
	const secondAt = Date.now() - started;
	check(
		survived && second.includes('keep-alive') && secondAt >= QUIET_MS,
		`a quiet period of ${QUIET_MS} ms does not cut the stream (got the keep-alive at ${secondAt} ms)`
	);
	// A second stream, abandoned by the client, must free the upstream request.
	upstreamClosed = false;
	const abandoned = new AbortController();
	const second_response = await fetch(`http://127.0.0.1:${proxyPort}${STREAM}`, {
		signal: abandoned.signal
	});
	await second_response.body!.getReader().read();
	abandoned.abort();
	await Bun.sleep(1_000);
	check(upstreamClosed, 'a client that goes away closes the upstream request');
	check(upstreamHits === 2, 'each client request made exactly one upstream request');
} finally {
	proxy.kill();
	stub.stop(true);
	rmSync(dir, { recursive: true, force: true });
}

if (failures > 0) {
	console.error(`${failures} check(s) failed`);
	process.exit(1);
}
console.log('e2e-web-proxy streaming: ok');
