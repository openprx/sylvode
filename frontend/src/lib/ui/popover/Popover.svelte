<script lang="ts">
	import { tick, type Snippet } from 'svelte';
	import { pushLayer } from '../foundations/layers';
	import { portal } from '../foundations/portal';
	import { autoPosition, type Placement } from '../foundations/position';
	import { captureFocus, tabbables } from '../foundations/focus';

	interface Props {
		open: boolean;
		/** The element the popover hangs from; it gets focus back when the popover closes. */
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Accessible name; or use `labelledby`. */
		label?: string;
		labelledby?: string;
		role?: 'dialog' | 'menu' | 'listbox' | 'none';
		placement?: Placement;
		width?: number;
		/** Selector of the element that takes focus on open; default is the first tabbable one. */
		initialFocus?: string;
		/** Move focus in on open; menus and listboxes do it themselves. Default true. */
		autofocus?: boolean;
		padding?: 'default' | 'menu' | 'none';
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
		role = 'dialog',
		placement = 'bottom-start',
		width = 320,
		initialFocus,
		autofocus = true,
		padding = 'default',
		id,
		testId,
		children
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
		bind:this={panel}
		{id}
		role={role === 'none' ? undefined : role}
		aria-label={label}
		aria-labelledby={labelledby}
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
		{@render children()}
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
	@keyframes ui-popover-in {
		from {
			opacity: 0;
		}
	}
</style>
