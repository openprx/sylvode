# Sylvode Frontend

The Sylvode frontend application, built with SvelteKit + TypeScript + Tailwind CSS + shadcn-svelte.

## Tech Stack

- **Framework:** SvelteKit 2.x (Svelte 5)
- **Language:** TypeScript
- **Runtime:** Bun 1.3+
- **UI library:** shadcn-svelte
- **Styling:** Tailwind CSS v4
- **Build tool:** Vite 7

## Quick Start

### Install dependencies

```bash
bun install
```

### Development server

```bash
bun run dev
```

Open http://localhost:5173

### Production build

```bash
bun run build
```

### Preview the production build

```bash
bun run preview
```

## Project Structure

```
src/
├── lib/
│   ├── api/              # API clients
│   │   ├── client.ts     # Base HTTP client
│   │   ├── auth.ts       # Authentication API
│   │   ├── workspaces.ts # Workspace API
│   │   ├── projects.ts   # Project API
│   │   ├── issues.ts     # Work item API
│   │   └── notifications.ts # Notification API
│   ├── stores/           # Svelte state management
│   │   ├── auth.ts       # Authentication state
│   │   └── toast.ts      # Toast notifications
│   └── components/       # Reusable components
│       └── Toast.svelte  # Toast notification component
├── routes/               # Page routes
│   ├── (auth)/           # Authentication route group
│   │   └── auth/login/   # Login page
│   └── (app)/            # Application route group (requires authentication)
│       ├── inbox/        # Notification center
│       └── workspace/    # Workspace
│           ├── [workspaceId]/
│           │   └── projects/
│           │       ├── +page.svelte         # Project list
│           │       └── [projectId]/
│           │           ├── +page.svelte     # Project detail
│           │           ├── issues/          # Work item list / detail
│           │           ├── board/           # Board view
│           │           └── cycles/          # Cycle (iteration) management
│           └── +page.svelte                 # Workspace selection
└── app.css               # Global styles

```

## Core Pages

### Implemented (9)

1. **Login page** - `/auth/login`
2. **Workspace selection** - `/workspace`
3. **Project list** - `/workspace/:workspaceId/projects`
4. **Project detail** - `/workspace/:workspaceId/projects/:projectId`
5. **Work item list** - `/workspace/:workspaceId/projects/:projectId/issues`
6. **Work item detail** - `/workspace/:workspaceId/projects/:projectId/issues/:issueId`
7. **Board view** - `/workspace/:workspaceId/projects/:projectId/board`
8. **Cycles** - `/workspace/:workspaceId/projects/:projectId/cycles` (placeholder page)
9. **Notification center** - `/inbox`

## Features

### Completed

- [x] API client wrapper (unified error handling, automatic token management)
- [x] Authentication flow (login / logout / token refresh)
- [x] Route guard (AuthGuard)
- [x] State management (Svelte stores)
- [x] Toast notification system
- [x] Responsive design (desktop / tablet / mobile)
- [x] Loading / Error / Empty state handling
- [x] Work item CRUD operations
- [x] Comments
- [x] Board view
- [x] Notification center

### Remaining

- [ ] Drag and drop (board)
- [ ] Full implementation of Cycles management
- [ ] Image upload
- [ ] Markdown editor
- [ ] Real-time notifications (WebSocket)
- [ ] Search
- [ ] Accessibility improvements (fix a11y warnings)

## Environment Variables

Create a `.env` file:

```bash
# API base URL
VITE_API_BASE_URL=http://localhost:8081
```

## API Integration

For the backend API documentation, see `docs/API_ENDPOINTS_PHASE3.md` in the repository root.

All API requests are sent through `apiClient` in `$lib/api/client.ts`, which handles:

- JWT token injection (Authorization header)
- Unified error response format
- Loading state management
- Token persistence in LocalStorage

## Style Guide

### Tailwind CSS

- Mobile-first
- Responsive breakpoints: `sm` (640px), `md` (768px), `lg` (1024px), `xl` (1280px)
- Color scheme: Slate (primary)

### Component Conventions

- Touch-friendly (buttons at least 44x44px)
- Tables switch to card view on mobile
- The sidebar collapses automatically on small screens

## Build and Deployment

### Production build

```bash
bun run build
# Output: .svelte-kit/output/
```

### Adapter configuration

The project currently uses `@sveltejs/adapter-auto`, which detects the deployment platform automatically:

- Node.js server
- Vercel
- Netlify
- Cloudflare Pages
- and others

To pin a specific adapter, edit `svelte.config.js`.

## Development Guide

### Add a new page

1. Create a directory and `+page.svelte` under `src/routes/`
2. If the page needs data loading, add `+page.ts`
3. If the page needs server-side data, add `+page.server.ts`

### Add an API method

Add the method to the matching module in `src/lib/api/` and send requests with `apiClient`.

### Add global state

Create a new store file in `src/lib/stores/`.

## FAQ

### Q: Why Bun instead of npm/pnpm?

A: Bun is faster; installing dependencies and running scripts is 2-10 times faster than npm.

### Q: How do I handle API connection problems?

A: In development, point `VITE_API_BASE_URL` in `.env` at the backend API, for example `http://localhost:8081`. In the production Docker Compose setup, the frontend forwards API requests through a same-origin nginx proxy.

### Q: The build reports a11y warnings?

A: These are accessibility warnings and do not break the build. Adding `aria-label` attributes is recommended to improve usability.

## License

Dual-licensed: [MIT](../LICENSE-MIT) or [Apache-2.0](../LICENSE-APACHE).
