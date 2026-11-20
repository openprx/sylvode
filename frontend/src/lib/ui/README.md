# lib/ui: design system primitives

Components of the redesign (specification: `DESIGN-AND-INTERACTION-SPEC.md`; batch A designs for
the keyboard policy and shared rules). Everything here is self-contained so several people can add
components in parallel without editing the same file.

## Layout

One folder per component, kebab-case, files in PascalCase for components:

```
lib/ui/
  foundations/        shared helpers (focus, focus trap, roving, layers, modal, position, portal, ids, messages, compose, typeahead, plural, visually-hidden)
  button/
    Button.svelte     the component
    button-state.ts   logic that can be tested without a DOM (optional)
    button.test.ts    behaviour test (runs with Bun)
    Gallery.svelte    its section of the test page /admin/ui
    locale.en.json    its messages, English
    locale.zh.json    its messages, Chinese
```

There is no barrel file: import a component by its path
(`import Button from '../button/Button.svelte'`, or `#lib/ui/button/Button.svelte` from outside),
so adding a component never touches a shared export list.

## Adding a component (five lines)

1. Create `lib/ui/<name>/` with `<Name>.svelte`.
2. Style with `var(--rf-*)` tokens in the component's `<style>`; no raw palette, hex, `dark:` or `rounded-sm|md|lg|xl`.
3. Add `Gallery.svelte` rendering each variant once with `data-gallery="<name>:<variant>"`; `/admin/ui` finds it by glob.
4. Add `locale.en.json` and `locale.zh.json` with keys under `ui.<name>.*` (or `states.*`); the app merges them.
5. Add `<name>.test.ts` (see Tests) and run `bun run test:ui`, `bun run design-lint`, `bun run check`.

## Conventions

- Svelte 5 runes and snippets, strict TypeScript, no `any`. Props extend the native element's attributes where there is one (`HTMLButtonAttributes`) and are spread onto it.
- Colours, sizes, radii, spacing, durations, z-index and shadows come from `--rf-*` variables (`src/tokens.css`). Names in the specification map one to one: `focus` is `var(--rf-focus)`.
- Focus: every focusable part sets its own `:focus-visible` rule: `outline: var(--rf-border-width-focus) solid var(--rf-focus)` with `outline-offset: -1px`; filled controls (primary and danger buttons, a checked checkbox, radio or switch) mark themselves `data-filled` and use `outline-offset: 2px`. The component sets the outline itself because the base layer's `outline-ring/50` would otherwise recolour the shared rule in `tokens.css`.
- Disabled is its own look (never opacity): the `surface` fill with `secondary` text (`text-fg-muted`; deviation D8, 6.19 / 6.01 on `surface`), the same in `Button`, `IconButton`, fields and raw `disabled:` utilities. It comes with a visible reason (`Button` has `disabledReason`). Disabled menu items stay focusable and are `aria-disabled`.
- Floating layers (popover, menu, tooltip, dialog) use the `portal` action, the `z-*` tokens, `pushLayer` for Escape and `captureFocus` for focus return. Escape closes only the topmost layer.
- Touch: under `@media (pointer: coarse)` the hit area grows to `--rf-touch-target` with padding.
- Keyboard: only keys a page can own in Chrome and Firefox. No Ctrl or Meta with digits 1-5, Tab, Shift+T, Shift+P or J.
- Every visible string goes through `$t('ui.<name>.<key>')`. Sample strings in `Gallery.svelte` do too.
- Logical CSS properties where direction matters (`text-align: start`, `margin-inline-*`).
- Caller handlers: a primitive that needs its own handler on a native event (`Checkbox` and `Radio` on `change`, `Button` on `click`) runs its own first and then the caller's (`foundations/compose.ts`), so an `onchange` passed to the component is never discarded. Do not put an internal handler after `{...rest}` without composing.
- Data attributes: `Popover`, `Menu`, `MenuItem` and `StateBlock` pass `data-*` props (`data-testid`, ...) to their element; `testId` on `Popover` and `Menu` stays and sets `data-popover`.
- Visually hidden text: add `import '../foundations/visually-hidden.css';` and the class `ui-sr-only`. Never write a local `.sr-only` rule.
- Plural wording: the app's i18n has no ICU plural syntax. A message that depends on a count is a group of keys by plural category (`ui.dataTable.selected.one`, `.other`) read with `plural($t, 'ui.dataTable.selected', count, $locale)` (`foundations/plural.ts`, categories from `Intl.PluralRules`; unknown language falls back to English). The count is passed as `{count}`. Chinese has one form, so `locale.zh.json` defines only `other`; the locale parity test ignores categories a language does not use but requires `other` everywhere. Never write `xOne` and `xMany` keys.

## Forms and the required marking

`form/Field.svelte` wraps any control and prints the label with the marking word; `TextField` and `Textarea` bring their own label, so they take the same two props (`requirement`, `marking`) and show the same word (`ui.form.required` / `ui.form.optional`, through `markingWord`). Both ways exist on purpose: use `TextField` or `Textarea` alone for a plain labelled field, and `Field` around controls that have no label of their own (selects, date fields, custom controls). This duplicates two lines per component and avoids the double label that `Field` around `TextField` would produce. `requirement: 'required'` also sets the native `required` attribute.

## Layers

- `Popover` takes `modal` (sets `aria-modal`), `describedby`, `sheetTitle` and any `data-*` prop for its surface. Below 640px it becomes a bottom sheet (specification 3.6): a 36 by 5 grabber, a centred title (`sheetTitle`, or `label` for a `dialog`), the body, and a full-width outlined Close button when there is a title. A popover without a title shows only the grabber.
- `Menu` has `header` (non-interactive caption inside the surface) and `describedby`. Items are `MenuItem`: `role` is `menuitem`, `menuitemradio` or `menuitemcheckbox` (with `checked`, which sets `aria-checked` and the leading check), `href` renders a real link (Cmd/Ctrl-click and middle-click open a new browser tab; the menu stays), `trailing` adds content at the end, `keepOpen` leaves the menu open after the handler. Type-ahead: a printable key focuses the next item whose label starts with it; typing quickly extends the prefix, a pause of 500 ms resets it.
- `Dialog` (and so `DestructiveDialog`) calls `enterModal()` (`foundations/modal.ts`) while open; `ToastRegion` listens and holds new toasts until the last modal closes. Any other layer that must make toasts wait can call `enterModal` too.

## States

`StateBlock` renders the whole `states.*` catalogue (T16): `empty`, `loading`, `noResults`, `noSearchResults`, `offline`, `loadFailed`, `forbidden`, `notFound`, `expired`, `identityChanged`. Parameters: `thing` (plural; `empty`, `loading`, `loadFailed`), `object` (singular; `notFound`), `grantor` and `place` (`forbidden`; `place` is also the target of the `notFound` action), `count` (`noResults`: applied filters, plural sentence; without it no body), `query` (`noSearchResults`). Without `thing` or `object` the generic wording is used. `message` replaces the body for a feature that says more, and `action` supplies the one button (its label defaults per kind). `DataTable` and `SidePanel` use it for their blocking states.

## Messages

The app dictionary is `src/lib/i18n/{en,zh}.json`; `src/lib/i18n/index.ts` merges every
`lib/ui/**/locale.<lang>.json` into it when the language loads. A key defined in two files with
different text throws, so two components cannot silently claim the same key. Keep keys under your
component's own prefix. `foundations/locale.*.json` holds only the generic `ui.gallery.*` sample strings.

## Tests

- `bun run test:ui` runs every `lib/ui/**/*.test.ts` with Bun (one process per file; the runner finds new files itself). Tests use `tests/support/harness.ts`. Pure logic (roving index, trap target, positioning, tooltip timing, button state) is tested directly; initial markup is rendered with `tests/support/svelte-ssr.ts`.
- There is no DOM in the unit setup, so key presses, focus placement and computed styles are checked in a real browser: `bun run test:ui-browser -- --base-url <url>` (script `scripts/ui-behaviour.mjs`) drives headless Chromium against `/admin/ui` with a signed-in admin. Add a check there when a component has keyboard behaviour; locate specimens by `data-gallery`.
- `bun run design-lint` must report 0 for `src/lib/ui` (`node scripts/design-lint.mjs --root src/lib/ui`).

## Gallery

`/admin/ui` (instance admins only, like the rest of `/admin`) is an internal test page, not a
showcase: each `Gallery.svelte` renders every variant once. A switch toggles a dark container so
derived tokens can be checked in the nested dark scope.

## Open points

- Hover and pressed on filled buttons (primary, danger) are not specified; they mix 12% and 22% of `--rf-bg` into the fill (`color-mix`). Danger-quiet mixes 10% and 18% of `--rf-danger` into `--rf-danger-bg`. Owner to confirm or replace.
- Focus ring colour: `app.css` base layer sets `outline-color: ring/50` on every element, which overrides the `tokens.css` focus rule; components re-declare the outline, so the global rule is still wrong for old components until Phase 3.
- A disabled button is a native `disabled` button, so it leaves the tab order; the reason text is visible and linked with `aria-describedby`.
- Tab inside a menu closes it and returns focus to the trigger.
- The tokens `--rf-layout-header-compact`, `--rf-z-toast` and `--rf-leading-metric` are PROPOSED; they are generated into `tokens.css` by `tools/gen_tokens.py` in the design-system folder (never edit the generated file).
- Popover becomes the sheet by viewport width only (below 640px), not by pointer type; the date picker design says a desktop pointer should never get the sheet at a small width.
- The tooltip action mounts its component into `document.body` (or the nearest themed container); `describes` decides between `aria-hidden` and `aria-describedby`.
- Dialog keeps a draft only if the draft lives outside the dialog content, or `keepMounted` is set.
- Dialog does not make the rest of the page `inert`; the focus trap and the scrim cover keyboard and pointer.
- Selection marker, compact density and the other later-phase items are not part of this slice.
