# Sylvode web UI

The Sylvode web UI is a SvelteKit application (Svelte 5, TypeScript, Tailwind CSS 4, Vite 8),
built with `@sveltejs/adapter-static` into a static single-page application. It talks to the
Sylvode API over HTTP and WebSocket. Installing the whole stack is described in
[docs/install.md](../docs/install.md).

## Requirements

- Bun 1.x. The `build` script also runs `node scripts/externalize-bootstrap.mjs`, so `node` must be
  on the `PATH` as well.

```bash
cd frontend
bun install --frozen-lockfile
```

## Development server

```bash
VITE_API_BASE_URL=http://localhost:8081 bun run dev
```

Vite serves the UI at `http://localhost:5173`. `VITE_API_BASE_URL` points it at a running API, here
the API port of the Compose stack. You can also put the line in `frontend/.env`:

```bash
VITE_API_BASE_URL=http://localhost:8081
```

Accounts are created through the API; see
[the first administrator account](../docs/install.md#the-first-administrator-account).

## Production build

```bash
bun run build
```

The output is written to `frontend/build/`. The build moves SvelteKit's inline start script into a
file of its own, so the UI runs under a Content Security Policy with `script-src 'self'`.

`bun run preview` serves the built files locally. `bun run check` runs the type checker.

## `VITE_API_BASE_URL`

The value is read at build time and compiled into the files.

| Value          | Effect                                                                                                                                                                                                                                           |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| unset or empty | API requests go to the origin the UI is served from, and the live editing WebSocket uses the same origin with `ws` or `wss`. Use this when nginx (or another proxy) forwards `/api` to the API, as in the Compose stack and the release archive. |
| a URL          | API requests go to that URL, and the WebSocket URL is derived from it by replacing `http` with `ws`.                                                                                                                                             |

Whatever the value, live editing works only from an origin listed in the API's
`flow.collab_allowed_origins`, for the development server `http://localhost:5173`; see
[docs/configuration.md](../docs/configuration.md#flow).

## Serving the build

`nginx.conf` is the server configuration of the UI image and of the `sylvode-frontend.tar.gz`
release archive. It serves `build/` with a fallback to `index.html`, forwards `/api` and `/uploads`
to the API at `api:8080`, sets the security headers and keeps credentials out of the access log.
Outside the Compose network, change the upstream address and the resolver include as described in
[docs/install.md](../docs/install.md#serve-the-web-ui). `nginx-static-only.conf` serves the files
without forwarding anything.

| File                  | Image it builds                                                                                                                 |
| --------------------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `Dockerfile`          | Installs the dependencies and builds the UI inside the image, then serves it with nginx. The Compose default.                   |
| `Dockerfile.prebuilt` | Serves an existing `build/` directory, for hosts without Bun. Select it with `SYLVODE_FRONTEND_DOCKERFILE=Dockerfile.prebuilt`. |

Reverse proxy requirements are listed in [docs/operations.md](../docs/operations.md#reverse-proxy).

## License

Dual-licensed: [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE).
