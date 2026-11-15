<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { get } from 'svelte/store';
	import { paneBounds, clampPaneWidth } from '#lib/flow/pane-width.js';
	import { authStore } from '#lib/stores/auth.js';
	let {
		side,
		label,
		children,
		scrollTop = 0
	}: { side: 'left' | 'right'; label: string; children: Snippet; scrollTop?: number } = $props();
	let containerWidth = $state(1400);
	const bounds = $derived(paneBounds(side, containerWidth));
	const initial = $derived(bounds.initial),
		minimum = $derived(bounds.min),
		maximum = $derived(bounds.max);
	let width = $state(256);
	let pane = $state<HTMLDivElement | null>(null);
	let dragging = $state(false);
	let storageKey = '';
	onMount(() => {
		width = initial;
		const container = pane?.parentElement?.parentElement;
		const observer = new ResizeObserver(() => {
			if (container) {
				containerWidth = container.clientWidth;
				width = clampPaneWidth(width, minimum, maximum);
			}
		});
		if (container) observer.observe(container);
		const scroller = pane?.querySelector('.pane-content');
		let desired = scrollTop;
		const restore = new ResizeObserver(() => {
			if (scroller && desired > 0) {
				scroller.scrollTop = desired;
				if (scroller.scrollTop >= desired - 1) desired = 0;
			}
		});
		const inner = pane?.querySelector('.pane-inner');
		if (inner) restore.observe(inner);
		const cancelRestore = () => (desired = 0);
		scroller?.addEventListener('wheel', cancelRestore, { passive: true });
		scroller?.addEventListener('pointerdown', cancelRestore);
		storageKey = `sv-flow-pane:${get(authStore).user?.id ?? 'anonymous'}:${side}`;
		try {
			const value = Number(localStorage.getItem(storageKey));
			if (value >= minimum && value <= maximum) width = value;
		} catch {
			/* Optional preference storage. */
		}
		return () => {
			observer.disconnect();
			restore.disconnect();
			scroller?.removeEventListener('wheel', cancelRestore);
			scroller?.removeEventListener('pointerdown', cancelRestore);
		};
	});
	function setWidth(value: number, save = true) {
		width = clampPaneWidth(value, minimum, maximum);
		if (save)
			try {
				localStorage.setItem(storageKey, String(width));
			} catch {
				/* Optional preference storage. */
			}
	}
	function down(event: PointerEvent) {
		if (event.button !== 0) return;
		event.preventDefault();
		const target = event.currentTarget as HTMLElement;
		target.setPointerCapture(event.pointerId);
		dragging = true;
	}
	function move(event: PointerEvent) {
		if (!dragging || !pane) return;
		const bounds = pane.getBoundingClientRect();
		setWidth(side === 'left' ? event.clientX - bounds.left : bounds.right - event.clientX, false);
	}
	function end() {
		if (dragging) {
			dragging = false;
			setWidth(width);
		}
	}
	function key(event: KeyboardEvent) {
		if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
			event.preventDefault();
			setWidth(
				width +
					(event.key === 'ArrowRight' ? 1 : -1) *
						(side === 'left' ? 1 : -1) *
						(event.shiftKey ? 32 : 8)
			);
		} else if (event.key === 'Home') {
			event.preventDefault();
			setWidth(minimum);
		} else if (event.key === 'End') {
			event.preventDefault();
			setWidth(maximum);
		} else if (event.key === 'Enter') {
			event.preventDefault();
			setWidth(initial);
		}
	}
</script>

<div
	class="flow-pane"
	class:right={side === 'right'}
	style={`--pane-width:${width}px`}
	bind:this={pane}
>
	<div class="pane-content"><div class="pane-inner">{@render children()}</div></div>
	<!-- Focusable separators are interactive ARIA range controls; Svelte classifies the role as static. -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
	<div
		role="separator"
		tabindex="0"
		aria-label={label}
		aria-orientation="vertical"
		aria-valuemin={minimum}
		aria-valuemax={maximum}
		aria-valuenow={width}
		title={label}
		data-testid={`flow-${side}-separator`}
		class="resize-handle"
		class:dragging
		onpointerdown={down}
		onpointermove={move}
		onpointerup={end}
		onpointercancel={end}
		onlostpointercapture={end}
		onkeydown={key}
		ondblclick={() => setWidth(initial)}
	></div>
</div>

<style>
	.flow-pane {
		display: flex;
		position: relative;
		flex-shrink: 0;
		width: 256px;
		min-height: 0;
		max-width: 85vw;
	}
	.flow-pane:not(.right) .pane-inner {
		height: 100%;
	}
	.pane-content {
		min-width: 0;
		width: 100%;
		height: 100%;
		overflow: auto;
	}
	.resize-handle {
		display: none;
	}
	@container flowworkspace (min-width: 900px) {
		.flow-pane {
			width: var(--pane-width);
			max-width: 38vw;
		}
		.resize-handle {
			display: block;
			position: absolute;
			right: -3px;
			top: 0;
			bottom: 0;
			width: 7px;
			z-index: 35;
			cursor: col-resize;
			touch-action: none;
		}
		.right .resize-handle {
			right: auto;
			left: -3px;
		}
		.resize-handle:hover,
		.resize-handle:focus-visible,
		.resize-handle.dragging {
			background: #3b82f6;
			outline: none;
		}
	}
</style>
