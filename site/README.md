# Sylvode front site (skeleton)

Public-facing site client of the low-code platform. This batch holds the skeleton only: build setup,
router shell, design tokens, a small UI kit and a component gallery. No site API endpoints exist yet.

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

## Structure

- `src/main.tsx`, `src/app.tsx`: mount, router (`/`, `/gallery`, `/p/:slug`, 404) and shell.
- `src/routes/`: route components. `gallery.tsx` shows every component in light and dark.
- `src/components/ui/`: Button, Input, Textarea, Select, Checkbox, Card, Badge, Modal, Table, Toast,
  ThemeToggle, Icon (own code over Kobalte where useful; no shadcn package).
- `src/styles/tokens.css`: design tokens. `src/styles/app.css`: Tailwind import and `@theme inline` mapping.
- `src/lib/api.ts`: typed fetch helper.

## Configuration

- `VITE_SV_SITE_API`: API base URL, default `/api/site/v1` (same origin).
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
