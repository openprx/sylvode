<script lang="ts">
	import { tick, type Snippet } from 'svelte';
	import { t } from 'svelte-i18n';
	import { uid } from '../foundations/ids';
	import { pushLayer } from '../foundations/layers';
	import { portal } from '../foundations/portal';
	import { autoPosition, type Placement } from '../foundations/position';
	import { captureFocus, tabbables } from '../foundations/focus';

	/** Data attributes (`data-testid`, ...) are passed to the surface element. */
	type DataAttributes = { [K in `data-${string}`]?: string | undefined };

	interface Props extends DataAttributes {
		open: boolean;
		/** The element the popover hangs from; it gets focus back when the popover closes. */
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Accessible name; or use `labelledby`. */
		label?: string;
		labelledby?: string;
		describedby?: string;
		role?: 'dialog' | 'menu' | 'listbox' | 'none';
		placement?: Placement;
		width?: number;
		/** Selector of the element that takes focus on open; default is the first tabbable one. */
		initialFocus?: string;
		/** Move focus in on open; menus and listboxes do it themselves. Default true. */
		autofocus?: boolean;
		padding?: 'default' | 'menu' | 'none';
		/** Sets `aria-modal` on the surface (a dialog that takes the user's attention). Default off. */
		modal?: boolean;
		/**
		 * Title of the bottom sheet under 640px, centred above the body. Defaults to `label` for a
		 * `dialog`; without any title the sheet shows only the grabber.
		 */
		sheetTitle?: string;
		id?: string;
		testId?: string;
		children: Snippet;
	}

	let {
		open,
		anchor,
		onclose,
		label,
		labelledby,
		describedby,
		role = 'dialog',
		placement = 'bottom-start',
		width = 320,
		initialFocus,
		autofocus = true,
		padding = 'default',
		modal,
		sheetTitle,
		id,
		testId,
		children,
		...rest
	}: Props = $props();

	let panel = $state<HTMLElement | null>(null);
	let restoreOpener: (() => void) | null = null;
	let wasOpen = false;

	// Runs before the DOM updates, so the element that opened the popover still has focus.
	$effect.pre(() => {
		if (open && !wasOpen) restoreOpener = captureFocus();
		wasOpen = open;
	});

	/** The anchor, or the first tabbable element inside it when the anchor is a wrapper. */
	function anchorTarget(): HTMLElement | null {
		if (!anchor) return null;
		return anchor.tabIndex >= 0 ? anchor : (tabbables(anchor)[0] ?? null);
	}
	let sheet = $state(false);
	const titleId = uid('popover-title');
	const heading = $derived(sheetTitle ?? (role === 'dialog' ? label : undefined));

	// Below 640px a floating layer becomes a bottom sheet (specification 3.6).
	$effect(() => {
		if (!open) return;
		const media = window.matchMedia('(max-width: 639px)');
		sheet = media.matches;
		const update = () => (sheet = media.matches);
		media.addEventListener('change', update);
		return () => media.removeEventListener('change', update);
	});

	$effect(() => {
		if (!open || !panel) return;
		const node = panel;
		const stopLayer = pushLayer({ onEscape: () => onclose() });
		const stopPosition = anchor && !sheet ? autoPosition(anchor, node, { placement }) : () => {};

		function onPointerDown(event: PointerEvent) {
			const target = event.target as Node | null;
			if (!target || node.contains(target) || anchor?.contains(target)) return;
			onclose();
		}
		document.addEventListener('pointerdown', onPointerDown, true);

		void tick().then(() => {
			if (!autofocus) return;
			const target =
				(initialFocus ? node.querySelector<HTMLElement>(initialFocus) : null) ??
				node.querySelector<HTMLElement>('[data-autofocus]') ??
				tabbables(node)[0] ??
				node;
			target.focus();
		});

		return () => {
			document.removeEventListener('pointerdown', onPointerDown, true);
			stopLayer();
			stopPosition();
			// Focus that was inside the closed layer falls to the page; give it back to the anchor.
			const restore = restoreOpener;
			restoreOpener = null;
			void tick().then(() => {
				const active = document.activeElement;
				if (active && active !== document.body) return;
				if (restore) restore();
				if (document.activeElement === document.body) anchorTarget()?.focus();
			});
		};
	});

	function onKeydown(event: KeyboardEvent) {
		// Tab leaves a menu: it closes and focus goes back to the trigger (menu pattern).
		if (role === 'menu' && event.key === 'Tab') {
			event.preventDefault();
			onclose();
		}
	}

	function onFocusOut(event: FocusEvent) {
		const next = event.relatedTarget as Node | null;
		// Tabbing out of a non-modal popover closes it, like leaving a menu.
		if (next && !panel?.contains(next) && !anchor?.contains(next)) onclose();
	}
</script>

{#if open}
	<div
		{...rest}
		bind:this={panel}
		{id}
		role={role === 'none' ? undefined : role}
		aria-label={label}
		aria-labelledby={labelledby}
		aria-describedby={describedby}
		aria-modal={modal ? 'true' : undefined}
		tabindex="-1"
		class="ui-popover pad-{padding}"
		class:sheet
		style:--ui-popover-width="{width}px"
		data-ui-layer="popover"
		data-popover={testId}
		use:portal={anchor}
		onkeydown={onKeydown}
		onfocusout={onFocusOut}
	>
		{#if sheet}
			<div class="grabber" aria-hidden="true"></div>
			{#if heading}<h2 class="sheet-title" id={titleId}>{heading}</h2>{/if}
		{/if}
		{@render children()}
		{#if sheet && heading}
			<div class="sheet-foot">
				<button type="button" class="sheet-close" onclick={() => onclose()}>
					{$t('ui.popover.close')}
				</button>
			</div>
		{/if}
	</div>
{/if}

<style>
	.ui-popover {
		position: fixed;
		z-index: var(--rf-z-overlay);
		left: 0;
		top: 0;
		box-sizing: border-box;
		min-width: min(var(--ui-popover-width), calc(100vw - 2rem));
		max-width: calc(100vw - 2rem);
		max-height: min(70dvh, 32rem);
		overflow: auto;
		border: var(--rf-border-width) solid var(--rf-overlay-border);
		border-radius: var(--rf-radius-menu);
		background: var(--rf-overlay);
		color: var(--rf-text);
		box-shadow: var(--rf-shadow-overlay);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		animation: ui-popover-in var(--rf-duration-base) var(--rf-ease-standard);
	}
	.pad-default {
		padding: var(--rf-space-4) var(--rf-space-5);
	}
	.pad-menu {
		padding: var(--rf-space-2);
	}
	.pad-none {
		padding: 0;
	}
	.ui-popover:focus-visible {
		outline: none;
	}
	.sheet {
		left: 0 !important;
		right: 0;
		top: auto !important;
		bottom: 0;
		max-width: none;
		max-height: 80dvh;
		border-width: var(--rf-border-width) 0 0;
		border-radius: var(--rf-radius-sheet) var(--rf-radius-sheet) 0 0;
		box-shadow: var(--rf-shadow-sheet);
		padding-bottom: max(var(--rf-space-5), env(safe-area-inset-bottom));
	}
	.grabber {
		width: 36px;
		height: 5px;
		margin: var(--rf-space-4) auto var(--rf-space-3);
		border-radius: var(--rf-radius-full);
		background: var(--rf-line-strong);
	}
	.sheet-title {
		margin: 0;
		padding: var(--rf-space-2) var(--rf-space-5) var(--rf-space-4);
		text-align: center;
		font-size: var(--rf-size-subheading);
		font-weight: 600;
		line-height: var(--rf-leading-title);
		color: var(--rf-text);
	}
	.sheet-foot {
		padding: var(--rf-space-4) var(--rf-space-5) 0;
	}
	.sheet-close {
		width: 100%;
		min-height: var(--rf-touch-target);
		border: var(--rf-border-width) solid var(--rf-text);
		border-radius: var(--rf-radius-button);
		background: var(--rf-surface-white);
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-button);
		cursor: pointer;
	}
	.sheet-close:active {
		background: var(--rf-pressed);
	}
	.sheet-close:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	@keyframes ui-popover-in {
		from {
			opacity: 0;
		}
	}
</style>
