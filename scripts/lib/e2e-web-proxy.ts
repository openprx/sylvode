// Same-origin proxy for the browser E2E stack (scripts/e2e-web-stack.sh). Run with Bun.
//
// Serves the static frontend build and forwards every /api/* request to the API, the way
// frontend/nginx.conf does in the compose stack:
// - plain HTTP requests are forwarded with their headers (Host is dropped);
// - WebSocket upgrades (the Flow collab socket) are bridged to the API's ws:// endpoint and the
//   browser's Origin header is passed through, because the API checks it against
//   [flow].collab_allowed_origins;
// - the delegated event stream (GET /api/v1/oauth/events/stream, server-sent events) is passed
//   through chunk by chunk, with the per-connection idle timeout switched off (it only sends a
//   keep-alive comment every 20 s, and Bun's default 10 s idle timeout would cut it), and the
//   upstream request is dropped when the browser goes away so the API frees the stream's place.
// - the OAuth discovery documents the API serves at the origin root
//   (/.well-known/oauth-authorization-server and /.well-known/oauth-protected-resource/api) go to
//   the API as well; /.well-known/oauth-protected-resource/mcp goes to the MCP server when
//   E2E_PROXY_MCP_ORIGIN is set (it is not served otherwise: frontend/nginx.conf has no MCP proxy);
// - everything under /oauth/ (the consent page) is the SPA with the framing and caching headers
//   frontend/nginx.conf sends there: frame-ancestors 'none', X-Frame-Options: DENY, no-store and
//   no referrer.
// Unknown paths fall back to index.html (SPA routing).
//
// Inputs (all required): E2E_PROXY_BUILD_DIR, E2E_PROXY_API_ORIGIN (http://host:port),
// E2E_PROXY_PORT. E2E_PROXY_HOST defaults to 127.0.0.1. Optional: E2E_PROXY_MCP_ORIGIN.

function required(name: string): string {
	const value = process.env[name];
	if (!value) {
		console.error(`e2e-web-proxy: ${name} is not set`);
		process.exit(2);
	}
	return value;
}

const BUILD_DIR = required('E2E_PROXY_BUILD_DIR');
const API_ORIGIN = required('E2E_PROXY_API_ORIGIN');
const PORT = Number(required('E2E_PROXY_PORT'));
const HOST = process.env.E2E_PROXY_HOST || '127.0.0.1';
const WS_API_ORIGIN = API_ORIGIN.replace(/^http/, 'ws');
const EVENT_STREAM_PATH = '/api/v1/oauth/events/stream';
const MCP_ORIGIN = process.env.E2E_PROXY_MCP_ORIGIN || '';
// Served by the API at the origin root (contract 11.11); forwarded like /api/*.
const API_WELL_KNOWN = new Set([
	'/.well-known/oauth-authorization-server',
	'/.well-known/oauth-protected-resource/api'
]);
const MCP_WELL_KNOWN = '/.well-known/oauth-protected-resource/mcp';
// The consent page's response headers (contract 13.4): the policy of frontend/nginx.conf with
// frame-ancestors 'none'.
const OAUTH_PAGE_HEADERS: Record<string, string> = {
	'Content-Security-Policy':
		"default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self' 'wasm-unsafe-eval'; connect-src 'self'; font-src 'self' data:; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
	'X-Frame-Options': 'DENY',
	'X-Content-Type-Options': 'nosniff',
	'Cache-Control': 'no-store',
	'Referrer-Policy': 'no-referrer'
};

type Frame = string | ArrayBuffer | Uint8Array;
type SocketData = {
	target: string;
	origin: string | null;
	protocols: string | null;
	upstream?: WebSocket;
	queue: Frame[];
};

// Plain HTTP forwarding with the browser's headers (Host dropped). The upstream request ends when
// the browser's request ends, so a never-ending response (an event stream) frees its place.
async function forward(req: Request, target: string): Promise<Response> {
	const headers = new Headers(req.headers);
	headers.delete('host');
	headers.set('accept-encoding', 'identity');
	const upstream = await fetch(target, {
		method: req.method,
		headers,
		body: req.method === 'GET' || req.method === 'HEAD' ? undefined : await req.arrayBuffer(),
		redirect: 'manual',
		signal: req.signal
	});
	const out = new Headers(upstream.headers);
	out.delete('content-encoding');
	out.delete('content-length');
	return new Response(upstream.body, { status: upstream.status, headers: out });
}

Bun.serve<SocketData, never>({
	hostname: HOST,
	port: PORT,
	maxRequestBodySize: 1024 * 1024 * 1024,
	async fetch(req, server) {
		const url = new URL(req.url);
		if (url.pathname === MCP_WELL_KNOWN && MCP_ORIGIN) {
			return forward(req, MCP_ORIGIN + url.pathname + url.search);
		}
		if (API_WELL_KNOWN.has(url.pathname)) {
			return forward(req, API_ORIGIN + url.pathname + url.search);
		}
		if (url.pathname.startsWith('/api/')) {
			if (req.headers.get('upgrade')?.toLowerCase() === 'websocket') {
				const upgraded = server.upgrade(req, {
					data: {
						target: WS_API_ORIGIN + url.pathname + url.search,
						origin: req.headers.get('origin'),
						protocols: req.headers.get('sec-websocket-protocol'),
						queue: []
					}
				});
				return upgraded ? undefined : new Response('websocket upgrade failed', { status: 400 });
			}
			if (url.pathname === EVENT_STREAM_PATH) {
				// 0 disables the idle timeout for this connection only.
				server.timeout(req, 0);
			}
			return forward(req, API_ORIGIN + url.pathname + url.search);
		}
		if (url.pathname.startsWith('/oauth/')) {
			return new Response(Bun.file(BUILD_DIR + '/index.html'), {
				headers: { 'Content-Type': 'text/html; charset=utf-8', ...OAUTH_PAGE_HEADERS }
			});
		}
		if (url.pathname !== '/' && !url.pathname.includes('..')) {
			const file = Bun.file(BUILD_DIR + url.pathname);
			if (await file.exists()) return new Response(file);
		}
		return new Response(Bun.file(BUILD_DIR + '/index.html'), {
			headers: { 'Content-Type': 'text/html; charset=utf-8' }
		});
	},
	websocket: {
		open(ws) {
			const headers: Record<string, string> = {};
			if (ws.data.origin) headers['Origin'] = ws.data.origin;
			const protocols = ws.data.protocols
				? ws.data.protocols.split(',').map((p) => p.trim())
				: undefined;
			// Bun's WebSocket client accepts request headers in its options object.
			const upstream = new WebSocket(ws.data.target, { headers, protocols } as unknown as string[]);
			upstream.binaryType = 'arraybuffer';
			ws.data.upstream = upstream;
			upstream.onopen = () => {
				for (const frame of ws.data.queue) upstream.send(frame);
				ws.data.queue = [];
			};
			upstream.onmessage = (event) =>
				ws.send(typeof event.data === 'string' ? event.data : new Uint8Array(event.data as ArrayBuffer));
			upstream.onclose = (event) => ws.close(event.code === 1005 ? 1000 : event.code, event.reason);
			upstream.onerror = () => ws.close(1011, 'upstream error');
		},
		message(ws, message) {
			const upstream = ws.data.upstream;
			if (upstream && upstream.readyState === WebSocket.OPEN) upstream.send(message);
			else ws.data.queue.push(message);
		},
		close(ws) {
			ws.data.upstream?.close();
		}
	}
});

console.log(`e2e-web-proxy: serving ${BUILD_DIR} on http://${HOST}:${PORT}, /api/* -> ${API_ORIGIN}`);
