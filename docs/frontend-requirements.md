# Frontend Requirements - Sylvode

## Tech Stack
- **Framework**: Svelte + SvelteKit
- **Language**: TypeScript
- **Runtime**: Bun (replaces Node.js)
- **UI library**: shadcn-svelte (recommended) or Skeleton
- **Styling**: Tailwind CSS

## Responsive Design Requirements

### Breakpoints
- **Mobile**: < 768px
- **Tablet**: 768px - 1023px
- **Desktop**: >= 1024px

### Tailwind Breakpoint Usage
```
sm:  640px  (small phone, landscape)
md:  768px  (tablet, portrait)
lg:  1024px (desktop / tablet landscape)
xl:  1280px (large desktop)
2xl: 1536px (extra-large screen)
```

### Implementation Notes

#### 1. Mobile First
- Default styles target mobile
- Scale up with the `md:` and `lg:` prefixes
- Example:
  ```html
  <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3">
  ```

#### 2. Sidebar
- **Desktop (lg+)**: fixed on the left
- **Mobile/tablet**: collapses into a hamburger menu, shown as an overlay
- Control visibility with `lg:block hidden`

#### 3. Table / List Views
- **Desktop**: standard table (`<table>`)
- **Mobile**: card view (stacked `<div>`)
- Example:
  ```html
  <!-- Desktop table -->
  <table class="hidden lg:table">...</table>
  
  <!-- Mobile cards -->
  <div class="lg:hidden space-y-2">
    <div class="card">...</div>
  </div>
  ```

#### 4. Touch-Friendly Interaction
- **Minimum button size**: 44px × 44px (Apple HIG standard)
- **Touch target spacing**: >= 8px
- **Example**:
  ```html
  <button class="min-h-[44px] min-w-[44px] p-3">
    <!-- icon -->
  </button>
  ```

#### 5. Navbar
- **Desktop**: horizontal navigation
- **Mobile**: hamburger menu or bottom navigation bar
- Consider `<nav>` + `lg:flex` + `flex-col lg:flex-row`

#### 6. Content Layout
- **Container max width**: `max-w-7xl` (1280px)
- **Padding**: `px-4 sm:px-6 lg:px-8`
- **Grid**: `grid-cols-1 md:grid-cols-2 lg:grid-cols-3`

#### 7. Font Size
- Mobile base: `text-sm` (14px)
- Desktop base: `md:text-base` (16px)
- Responsive headings: `text-xl md:text-2xl lg:text-3xl`

#### 8. Images / Media
- Use `object-cover` and `aspect-ratio`
- Responsive sizing: `w-full lg:w-1/2`

## Core Page List (8)

1. **Login/registration page** (`/auth`)
2. **Workspace selection page** (`/workspaces`)
3. **Project list page** (`/workspace/:id/projects`)
4. **Board view page** (`/project/:id/board`)
5. **Work item list page** (`/project/:id/issues`)
6. **Work item detail page** (`/issue/:id`)
7. **Sprint management page** (`/project/:id/sprints`)
8. **Settings page** (`/settings`)

## Codex Implementation Checklist

### Project Setup
- [ ] Initialize with `bun create svelte@latest`
- [ ] Install TypeScript dependencies
- [ ] Configure Tailwind CSS
- [ ] Install shadcn-svelte
- [ ] Configure the API client (based on fetch/axios)

### Responsive Implementation
- [ ] All pages support the 3 breakpoints
- [ ] Mobile testing passes (Chrome DevTools)
- [ ] Touch targets meet the 44px standard
- [ ] Tables switch to cards on mobile
- [ ] Sidebar collapse works

### Component Library
- [ ] Button component (minimum 44px)
- [ ] Card component
- [ ] Responsive Table/List component
- [ ] Sidebar/Navbar component
- [ ] Modal/Dialog component
- [ ] Form component (with validation)

### Performance
- [ ] Lazy route loading
- [ ] Image optimization (WebP/AVIF)
- [ ] On-demand CSS loading
- [ ] Lighthouse mobile score > 90

## API Integration

### Authentication
- Token storage: `localStorage` or `sessionStorage`
- Automatic refresh mechanism
- Automatic redirect to login on 401

### Data Fetching
- SvelteKit `load` functions
- Server-side rendering (SSR) support
- Error handling and loading states

### State Management
- Svelte Stores or Pinia (if needed)
- Global user state
- Workspace/project context

## Deployment Requirements
- Optimized build output (`bun build`)
- CDN support for static assets
- Environment variable configuration (`.env`)
- Docker containerization (optional)
