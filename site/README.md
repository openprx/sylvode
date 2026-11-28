# Sylvode front site (skeleton)

Public-facing site client of the low-code platform (contract C3 section 6, `sv.layout.v1` section 1): build
setup, router, design tokens, a small UI kit, the layout renderer for the 12 components, public field
renderers, visitor submit, "my records", operator sign in and the operator list/update view. A mock mode
lets it run before the backend (`/api/site/v1`) exists.

## Stack (exact versions)

solid-js 2.0.3, @solidjs/web 2.0.3 (JSX via `jsxImportSource: "@solidjs/web"`), @solidjs/router 2.0.1
(`createRouter({ routes })`, plain `<a>` links), @kobalte/core 2.0.0-alpha.5, lucide 1.79.0 (icon nodes
rendered by our `Icon`), tailwindcss + @tailwindcss/vite 4.3.5, vite 8.3.8, vite-plugin-solid
3.0.0-next.27, typescript 7.0.2 (strict, `noUncheckedIndexedAccess`), @biomejs/biome 2.6.9. Package
manager: Bun (`bun.lock`).

## Scripts

```bash
bun install --frozen-lockfile
bun run dev          # vite dev server
bun run build        # output in dist/
bun run typecheck    # tsc --noEmit
bun run lint         # biome check
bun run format       # biome format --write
```

## Base path

The app is built for the base `/site/` (served by the API at `/site` and `/site/{*path}`). Override with
`VITE_SV_SITE_BASE` (must start and end with `/`, e.g. `VITE_SV_SITE_BASE=/ bun run build`). The router,
every `<a href>` and full-page navigation use the same base (`src/lib/base.ts`); API calls stay absolute
at `/api/site/v1`. With `VITE_SV_SITE_ROUTING=hash` the router base is empty (routes live in the hash).

## Structure

- `src/main.tsx`, `src/app.tsx`: mount, router (`/`, `/gallery`, `/p/:slug`, 404) and shell.
- `src/routes/`: route components. `gallery.tsx` shows every component in light and dark.
- `src/components/ui/`: Button, Input, Textarea, Select, Checkbox, Card, Badge, Modal, Table, Toast,
  ThemeToggle, Icon (own code over Kobalte where useful; no shadcn package).
- `src/styles/tokens.css`: design tokens. `src/styles/app.css`: Tailwind import and `@theme inline` mapping.
- `src/lib/api.ts`: typed fetch helper.

## Application structure (L3.S2m)

- `src/lib/api.ts`, `src/lib/site/client.ts`: typed client of every `/api/site/v1/{slug}` endpoint (manifest,
  pages, field list, record list/read/submit/update, operator login/logout). Cookies `sv_site_v` and `sv_site_op`
  are set by the server (`credentials: "include"`); writes send `X-SV-Site: 1`. Real HTTP statuses; `ApiError`
  carries `status`, `errorCode`, `details`.
- Routing: the app's own routes are `/login`, `/mine/:binding`, `/operator/:binding`, `/gallery`; every other
  path (including `/` and `/p/:slug` = route `/<slug>`) is matched against the manifest pages
  (`src/routes/page.tsx`, static routes win over `:param` routes) and loaded with `GET /pages?route=`.
  Unknown routes show the 404 page. The site is chosen by `VITE_SV_SITE_SLUG` or `?site=<slug>`.
- `src/lib/layout/`, `src/components/layout/`: renderer of the 12 components, same layout JSON and same
  `data-layout-node` structure as the console (`frontend/src/lib/layout`). `when` and `{cel}` use the vendored
  rules mirror; unknown nodes render a neutral placeholder; every node sits in its own `Errored` boundary.
  Site data sources use `binding:<key>`; only `submit` (create/update) and `navigate` with `route` run.
- `src/lib/rules/`: **vendored copy** of `frontend/src/lib/forms/rules/` (the `frontend/` tree is not
  importable from `site/` without changing its build). The only edits are `.ts` import suffixes dropped and
  `noUncheckedIndexedAccess` guards; lint is off for that folder (`biome.json` override). Re-copy and re-apply
  these edits when the console mirror changes. Dependency `@marcbachmann/cel-js` 8.0.0 (no `eval`).
- `src/lib/fields/`, `src/components/field-input.tsx`, `field-value.tsx`: public renderers. Editable: text,
  phone, email, address, textarea, rich_text (plain textarea), number, integer, amount, rating, progress, date,
  datetime, single/multi select, boolean. `member`, `relation`, `child_table`, files, images, `signature`,
  `location`, `scan` are blocked (nothing is shown or editable). Client validation is a hint (required and the
  built-in rules of `rules_client`); the server C2 report is the verdict and is shown per field.

## Mock mode

`VITE_SV_SITE_MOCK=1` installs a fixture-backed `fetch` (`src/lib/site/mock/`, shapes of contract section 6.6,
real statuses; state in `sessionStorage`). The import sits behind that build-time flag, so a normal build
contains no fixture (checked by grepping `dist/`). Operator login of the mock: `operator@demo.test` /
`mock-operator` (exists nowhere else). Mock site slug `demo`.

```bash
VITE_SV_SITE_MOCK=1 VITE_SV_SITE_SLUG=demo bun run build --outDir <dir> --emptyOutDir
bunx vite preview --outDir <dir> --port <port> --strictPort     # or: VITE_SV_SITE_MOCK=1 VITE_SV_SITE_SLUG=demo bun run dev
```

Mock pages: `/` (table of own requests), `/requests/new` (visitor form), `/showcase` (all 12 components, `when`,
`{cel}`, an unknown node), `/requests/<id>` (operator page, update form), `/mine/requests`, `/operator/requests`.

## Configuration

- `VITE_SV_SITE_API`: API base URL, default `/api/site/v1` (same origin).
- `VITE_SV_SITE_SLUG`: the site slug (else `?site=<slug>`). `VITE_SV_SITE_MOCK=1`: mock mode.
- `VITE_SV_SITE_ROUTING=hash`: use hash routing so a plain static file server can serve every route.
  Default is browser history, which needs the server to fall back to `index.html`
  (`vite preview` does).

## Tokens

Names and values follow the console tokens in `frontend/src/tokens.css` (subset, `--rf-*`):

- Colour: `--rf-bg --rf-nav --rf-surface --rf-surface-white --rf-selected --rf-overlay --rf-overlay-border
  --rf-input --rf-input-border --rf-text --rf-secondary --rf-control --rf-line --rf-line-strong
  --rf-primary --rf-on-primary --rf-focus --rf-badge --rf-on-badge --rf-scrim --rf-shadow
  --rf-success(-bg) --rf-on-success --rf-warning(-bg) --rf-danger(-bg) --rf-on-danger --rf-hover --rf-pressed`
- Type: `--rf-font-sans-stack --rf-font-mono-stack --rf-size-{caption,meta,secondary,button,body,subheading,title,page}
  --rf-weight-{regular,medium,semibold}`
- Shape and space: `--rf-radius-{xs,sm,md,lg,full,button,input,chip,card,dialog}`, `--rf-space-1..9`,
  `--rf-border-width*`, `--rf-shadow-{overlay,modal}`, `--rf-disabled-opacity`, `--rf-control-height`.

Themes: light by default; dark under `[data-theme="dark"]` or `prefers-color-scheme: dark` unless the page
is pinned with `[data-theme="light"]`. The toggle stores the choice in `localStorage` (`sv-site-theme`).
Tailwind maps them with `@theme inline` (for example `bg-surface`, `text-secondary`, `rounded-card`,
`text-small` = `--rf-size-secondary`), so nested `[data-theme]` containers re-resolve correctly.
Note: modal and toast render in a portal under `<body>`, so they follow the page theme, not a nested one.
