<script lang="ts">
	/**
	 * A small anchored popover: positioned under (or above) its anchor, closed by an outside
	 * click or Esc, focus moved in on open and back to the anchor on close. On narrow screens it
	 * becomes a bottom sheet. Used by the view bar's switcher, menus and popovers.
	 */
	import { onDestroy, tick, type Snippet } from 'svelte';

	interface Props {
		open: boolean;
		anchor: HTMLElement | null;
		onclose: () => void;
		/** Accessible name; or use `labelledby`. */
		label?: string;
		labelledby?: string;
		role?: 'dialog' | 'menu' | 'listbox' | 'none';
		align?: 'start' | 'end';
		width?: number;
		/** Whether the first focusable child receives focus on open (menus focus their own item). */
		autofocus?: boolean;
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
		align = 'start',
		width = 320,
		autofocus = true,
		id,
		testId,
		children
	}: Props = $props();

	let panel = $state<HTMLElement | null>(null);
	let style = $state('');
	let sheet = $state(false);

	const FOCUSABLE =
		'[data-autofocus], button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

	function place() {
		if (!anchor || !panel || typeof window === 'undefined') return;
		sheet = window.innerWidth < 640;
		if (sheet) {
			style = '';
			return;
		}
		const rect = anchor.getBoundingClientRect();
		const rtl = getComputedStyle(anchor).direction === 'rtl';
		const panelWidth = Math.min(Math.max(width, panel.offsetWidth), window.innerWidth - 32);
		const startEdge = (align === 'start') !== rtl;
		let left = startEdge ? rect.left : rect.right - panelWidth;
		left = Math.min(Math.max(16, left), window.innerWidth - panelWidth - 16);
		const below = window.innerHeight - rect.bottom;
		const height = panel.offsetHeight;
		const top =
			below < height + 16 && rect.top > below
				? Math.max(16, rect.top - height - 6)
				: rect.bottom + 6;
		style = `top:${Math.round(top)}px;left:${Math.round(left)}px;min-width:min(${width}px, calc(100vw - 2rem));`;
	}

	function onPointerDown(event: PointerEvent) {
		const target = event.target as Node | null;
		if (!target || panel?.contains(target) || anchor?.contains(target)) return;
		onclose();
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.key !== 'Escape') return;
		event.preventDefault();
		event.stopPropagation();
		onclose();
		anchor?.focus();
	}

	function onFocusOut(event: FocusEvent) {
		const next = event.relatedTarget as Node | null;
		// Tabbing out of a non-modal popover closes it, like leaving a menu.
		if (next && !panel?.contains(next) && !anchor?.contains(next)) onclose();
	}

	let wasOpen = false;
	$effect(() => {
		if (open && !wasOpen) {
			wasOpen = true;
			void tick().then(() => {
				place();
				if (autofocus) panel?.querySelector<HTMLElement>(FOCUSABLE)?.focus();
			});
			document.addEventListener('pointerdown', onPointerDown, true);
			window.addEventListener('resize', place);
			window.addEventListener('scroll', place, true);
		} else if (!open && wasOpen) {
			wasOpen = false;
			detach();
			if (panel?.contains(document.activeElement) || document.activeElement === document.body)
				anchor?.focus();
		}
	});

	function detach() {
		if (typeof document === 'undefined') return;
		document.removeEventListener('pointerdown', onPointerDown, true);
		window.removeEventListener('resize', place);
		window.removeEventListener('scroll', place, true);
	}

	onDestroy(detach);
</script>

{#if open}
	<div
		bind:this={panel}
		{id}
		role={role === 'none' ? undefined : role}
		aria-label={label}
		aria-labelledby={labelledby}
		tabindex="-1"
		class={sheet
			? 'sv-popover fixed inset-x-0 bottom-0 z-50 max-h-[80dvh] overflow-auto rounded-t-xl border-t border-slate-200 bg-white p-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:ring-1 dark:ring-slate-700'
			: 'sv-popover fixed z-50 max-h-[min(70dvh,32rem)] max-w-[calc(100vw-2rem)] overflow-auto rounded-lg border border-slate-200 bg-white p-2 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:ring-1 dark:ring-slate-700'}
		{style}
		data-popover={testId}
		onkeydown={onKeydown}
		onfocusout={onFocusOut}
	>
		{@render children()}
	</div>
{/if}

<style>
	.sv-popover {
		animation: sv-popover-in 120ms cubic-bezier(0.2, 0, 0, 1);
		transform-origin: top;
	}

	@keyframes sv-popover-in {
		from {
			opacity: 0;
			transform: scale(0.98);
		}
		to {
			opacity: 1;
			transform: scale(1);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.sv-popover {
			animation: sv-popover-fade 80ms linear;
		}

		@keyframes sv-popover-fade {
			from {
				opacity: 0;
			}
			to {
				opacity: 1;
			}
		}
	}
</style>
