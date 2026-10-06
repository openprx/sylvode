# Sylvode Frontend Quick Start

## Start the development server

```bash
cd frontend
bun run dev
```

Open: http://localhost:5173

## Test account (backend must be running)

There is no preconfigured account: on first run, register an account at http://localhost:5173, or run
`scripts/bootstrap-restaurant-demo.sh` to create a demo account (see the README in the repository root).

## Core page routes

| Page | Route | Description |
|------|------|------|
| Login | `/auth/login` | Email and password login |
| Workspace home | `/workspace` | Workspace selection |
| Project list | `/workspace/:id/projects` | Project management |
| Project detail | `/workspace/:id/projects/:pid` | Project overview |
| Issue list | `/workspace/:id/projects/:pid/issues` | Work item management |
| Issue detail | `/workspace/:id/projects/:pid/issues/:iid` | Work item detail |
| Board view | `/workspace/:id/projects/:pid/board` | Kanban board |
| Cycles | `/workspace/:id/projects/:pid/cycles` | Sprint management |
| Notification center | `/inbox` | Notifications |

## Environment Variables

Create a `.env` file:

```bash
VITE_API_BASE_URL=http://localhost:8081
```

## Build Commands

```bash
# Development
bun run dev

# Build
bun run build

# Preview
bun run preview

# Type check
bun run check

# Lint
bunx eslint .

# Format
bunx prettier --write .
```

## FAQ

### Q: Startup fails?

A: Make sure Bun 1.3+ is installed:

```bash
curl -fsSL https://bun.sh/install | bash
```

### Q: API requests fail?

A: Check that `VITE_API_BASE_URL` in `.env` is correct and that the backend service is running.

### Q: Blank page after login?

A: Open the browser console and look for API errors. Confirm that the backend database has been migrated.

## Development Docs

- [SvelteKit docs](https://kit.svelte.dev/docs)
- [Tailwind CSS docs](https://tailwindcss.com/docs)
- [shadcn-svelte docs](https://www.shadcn-svelte.com/)

## Next Steps

1. Start the backend API service
2. Run the database migrations
3. Start the frontend development server
4. Open http://localhost:5173 and test the features
