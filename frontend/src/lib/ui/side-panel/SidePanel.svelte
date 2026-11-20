<script lang="ts">
	import { t } from 'svelte-i18n';
	import { tick, type Snippet } from 'svelte';
	import ChevronUp from '@lucide/svelte/icons/chevron-up';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import X from '@lucide/svelte/icons/x';
	import Info from '@lucide/svelte/icons/info';
	import Lock from '@lucide/svelte/icons/lock';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import WifiOff from '@lucide/svelte/icons/wifi-off';
	import IconButton from '../icon-button/IconButton.svelte';
	import Menu from '../menu/Menu.svelte';
	import { tooltip } from '../tooltip/tooltip';
	import { focusTrap } from '../foundations/focus-trap';
	import { uid } from '../foundations/ids';
	import {
		PANEL_MIN,
		clampWidth,
		dragWidth,
		maxWidth,
		panelMode,
		separatorWidth,
		type PanelMode,
		type PanelStatus
	} from './side-panel';

	interface Props {
		title: string;
		identifier?: string;
		status: PanelStatus;
		/** Replaces the default sentence of a failed, forbidden or missing status. */
		statusMessage?: string;
		/** Hosted in a frame: the host draws header, close and Escape; only body and footer render. */
		hosted?: boolean;
		width: number;
		onresize: (px: number) => void;
		onclose: () => void;
		onprev?: () => void;
		onnext?: () => void;
		prevDisabledReason?: string;
		nextDisabledReason?: string;
		fullPageHref?: string;
		changedElsewhere?: boolean;
		offline?: boolean;
		onrefresh?: () => void;
		onretry?: () => void;
		/** Called after Escape (and F6) so the list can take focus back. */
		onfocuslist?: () => void;
		onmodechange?: (mode: PanelMode) => void;
		dirty?: boolean;
		/** Overrides for the measured container and window widths (tests, previews). */
		containerWidth?: number;
		windowWidth?: number;
		chip?: Snippet;
		/** Items of the overflow menu (MenuItem); the button is absent without it. */
		menu?: Snippet;
		children: Snippet;
		footer?: Snippet;
	}

	let {
		title,
		identifier,
		status,
		statusMessage,
		hosted = false,
		width,
		onresize,
		onclose,
		onprev,
		onnext,
		prevDisabledReason,
		nextDisabledReason,
		fullPageHref,
		changedElsewhere = false,
		offline = false,
		onrefresh,
		onretry,
		onfocuslist,
		onmodechange,
		dirty = false,
		containerWidth,
		windowWidth,
		chip,
		menu,
		children,
		footer
	}: Props = $props();

	const titleId = uid('panel-title');
	let root = $state<HTMLElement | null>(null);
	let heading = $state<HTMLElement | null>(null);
	let body = $state<HTMLElement | null>(null);
	let menuAnchor = $state<HTMLElement | null>(null);
	let menuOpen = $state(false);
	let measuredContainer = $state(0);
	let measuredWindow = $state(typeof window === 'undefined' ? 1280 : window.innerWidth);
	let dragging = $state(false);

	// Where focus was when the panel was created (the row that opened it).
	const opener = typeof document === 'undefined' ? null : document.activeElement;

	const container = $derived(containerWidth ?? measuredContainer);
	const viewport = $derived(windowWidth ?? measuredWindow);
	const mode = $derived<PanelMode>(
		container > 0 || windowWidth !== undefined ? panelMode(container || 1000, viewport) : 'docked'
	);
	const shown = $derived(container > 0 ? clampWidth(width, container) : width);
	const limit = $derived(maxWidth(container > 0 ? container : 1200));
	const sheet = $derived(mode === 'sheet' && !hosted);

	$effect(() => {
		const parent = root?.parentElement;
		if (!parent || containerWidth !== undefined) return;
		const observer = new ResizeObserver(() => (measuredContainer = parent.clientWidth));
		observer.observe(parent);
		measuredContainer = parent.clientWidth;
		return () => observer.disconnect();
	});
	$effect(() => {
		if (windowWidth !== undefined) return;
		const update = () => (measuredWindow = window.innerWidth);
		window.addEventListener('resize', update);
		return () => window.removeEventListener('resize', update);
	});
	$effect(() => {
		onmodechange?.(mode);
	});

	// The heading takes focus when the panel opens, so Escape and Tab start from the panel.
	$effect(() => {
		if (hosted) return;
		void tick().then(() => heading?.focus({ preventScroll: true }));
		return () => {
			// Closing: focus that was inside the panel goes back to the row that opened it.
			queueMicrotask(() => {
				const active = document.activeElement;
				if (
					(!active || active === document.body) &&
					opener instanceof HTMLElement &&
					opener.isConnected
				)
					opener.focus();
			});
		};
	});

	// A sheet is modal: everything around it is inert while it is open.
	$effect(() => {
		if (!sheet || !root?.parentElement) return;
		const parent = root.parentElement;
		const changed: HTMLElement[] = [];
		for (const sibling of Array.from(parent.children)) {
			if (sibling === root || !(sibling instanceof HTMLElement) || sibling.inert) continue;
			sibling.inert = true;
			changed.push(sibling);
		}
		return () => {
			for (const el of changed) el.inert = false;
		};
	});

	export function focusHeading(): void {
		heading?.focus();
	}
	export function contains(node: Node | null): boolean {
		return !!node && !!root?.contains(node);
	}

	function onKeydown(event: KeyboardEvent) {
		if (event.defaultPrevented) return;
		if (event.key === 'Escape' && !hosted) {
			event.preventDefault();
			onclose();
			onfocuslist?.();
			return;
		}
		if (event.key === 'F6' && !hosted && !event.altKey && !event.ctrlKey && !event.metaKey) {
			// Browsers keep F6 for the address bar; the Tab path stays the reliable one.
			if (onfocuslist) {
				event.preventDefault();
				onfocuslist();
			}
			return;
		}
		if (event.altKey && !event.shiftKey && !event.ctrlKey && !event.metaKey) {
			const handler =
				event.key === 'ArrowUp' && !prevDisabledReason
					? onprev
					: event.key === 'ArrowDown' && !nextDisabledReason
						? onnext
						: undefined;
			if (!handler) return;
			event.preventDefault();
			const inContent = !!body?.contains(document.activeElement);
			handler();
			if (inContent) void tick().then(() => heading?.focus());
		}
	}

	function onSeparatorKey(event: KeyboardEvent) {
		const rtl = root ? getComputedStyle(root).direction === 'rtl' : false;
		const next = separatorWidth(event.key, event.shiftKey, shown, container || 1200, viewport, rtl);
		if (next === null) return;
		event.preventDefault();
		onresize(next);
	}
	function startDrag(event: PointerEvent) {
		const parent = root?.parentElement;
		if (!parent || event.button !== 0) return;
		event.preventDefault();
		const target = event.currentTarget as HTMLElement;
		target.setPointerCapture(event.pointerId);
		dragging = true;
		const right = parent.getBoundingClientRect().right;
		const move = (e: PointerEvent) => onresize(dragWidth(e.clientX, right, container || 1200));
		const stop = () => {
			dragging = false;
			target.removeEventListener('pointermove', move);
			target.removeEventListener('pointerup', stop);
			target.removeEventListener('pointercancel', stop);
		};
		target.addEventListener('pointermove', move);
		target.addEventListener('pointerup', stop);
		target.addEventListener('pointercancel', stop);
	}

	const prevLabel = $derived(
		prevDisabledReason
			? `${$t('ui.sidePanel.previous')}, ${prevDisabledReason}`
			: $t('ui.sidePanel.previous')
	);
	const nextLabel = $derived(
		nextDisabledReason
			? `${$t('ui.sidePanel.next')}, ${nextDisabledReason}`
			: $t('ui.sidePanel.next')
	);
	const statusText = $derived(
		statusMessage ??
			(status === 'failed'
				? $t('ui.sidePanel.failed')
				: status === 'forbidden'
					? $t('ui.sidePanel.forbidden')
					: status === 'missing'
						? $t('ui.sidePanel.missing')
						: '')
	);
</script>

{#snippet tip(
	kind: 'neutral' | 'danger',
	text: string,
	action?: { label: string; run: () => void },
	icon?: 'info' | 'lock' | 'alert' | 'offline'
)}
	<div class="tip" class:danger={kind === 'danger'} role={kind === 'danger' ? 'alert' : 'status'}>
		<span class="tip-glyph">
			{#if icon === 'lock'}<Lock size={16} aria-hidden="true" />
			{:else if icon === 'alert'}<CircleAlert size={16} aria-hidden="true" />
			{:else if icon === 'offline'}<WifiOff size={16} aria-hidden="true" />
			{:else}<Info size={16} aria-hidden="true" />{/if}
		</span>
		<span class="tip-text">{text}</span>
		{#if action}<button type="button" class="tip-action" onclick={action.run}>{action.label}</button
			>{/if}
	</div>
{/snippet}

{#snippet content()}
	{#if changedElsewhere && onrefresh && status === 'ready'}
		{@render tip('neutral', $t('ui.sidePanel.changed'), {
			label: $t('ui.sidePanel.refresh'),
			run: onrefresh
		})}
	{/if}
	{#if offline}
		{@render tip('neutral', $t('ui.sidePanel.offline'), undefined, 'offline')}
	{/if}
	{#if status === 'failed'}
		{@render tip(
			'danger',
			statusText,
			onretry ? { label: $t('ui.sidePanel.retry'), run: onretry } : undefined,
			'alert'
		)}
	{:else if status === 'forbidden'}
		{@render tip('neutral', statusText, undefined, 'lock')}
	{:else if status === 'missing'}
		{@render tip('neutral', statusText)}
	{:else if status === 'loading'}
		<div class="skeletons" aria-hidden="true">
			{#each [0, 1, 2, 3, 4] as row (row)}<span class="skeleton"></span>{/each}
		</div>
	{:else}
		{@render children()}
	{/if}
{/snippet}

{#if hosted}
	<div
		class="hosted"
		aria-busy={status === 'loading' || undefined}
		data-panel-mode="hosted"
		bind:this={root}
	>
		<div class="body" bind:this={body}>{@render content()}</div>
		{#if footer && status === 'ready'}<div class="footer">{@render footer()}</div>{/if}
	</div>
{:else}
	{#if sheet}<div class="scrim" aria-hidden="true"></div>{/if}
	<svelte:element
		this={sheet ? 'div' : 'aside'}
		bind:this={root}
		class="ui-side-panel mode-{mode}"
		class:dragging
		style:width={sheet ? undefined : `${shown}px`}
		role={sheet ? 'dialog' : undefined}
		aria-modal={sheet ? 'true' : undefined}
		aria-labelledby={titleId}
		aria-busy={status === 'loading' || undefined}
		data-panel-mode={mode}
		data-ui="side-panel"
		tabindex={sheet ? -1 : undefined}
		use:focusTrap={{ active: sheet, initialFocus: '[data-heading]' }}
		onkeydown={onKeydown}
	>
		<!-- A window splitter: ArrowLeft/Right resize by 16 (Shift 64), Home and End jump to the limits. -->
		{#if !sheet}
			<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
			<div
				class="separator"
				role="separator"
				tabindex="0"
				aria-orientation="vertical"
				aria-label={$t('ui.sidePanel.resize')}
				aria-valuenow={shown}
				aria-valuemin={PANEL_MIN}
				aria-valuemax={limit}
				data-testid="side-panel-separator"
				onkeydown={onSeparatorKey}
				onpointerdown={startDrag}
			></div>
		{/if}
		<header class="header">
			<div class="heading">
				{#if identifier}<span class="identifier">{identifier}</span>{/if}
				<h2 id={titleId} bind:this={heading} tabindex="-1" data-heading class="title">
					{title}{#if dirty}<span class="dirty" role="img" aria-label={$t('ui.sidePanel.unsaved')}
						></span>{/if}
				</h2>
				{#if chip}<div class="chip">{@render chip()}</div>{/if}
			</div>
			<div class="tools">
				<IconButton
					icon={ChevronUp}
					label={prevLabel}
					disabled={!!prevDisabledReason || !onprev}
					onclick={onprev}
					shortcut="Alt ↑"
				/>
				<IconButton
					icon={ChevronDown}
					label={nextLabel}
					disabled={!!nextDisabledReason || !onnext}
					onclick={onnext}
					shortcut="Alt ↓"
				/>
				{#if fullPageHref}
					<!-- eslint-disable svelte/no-navigation-without-resolve -- full path built by the caller -->
					<a
						class="tool"
						href={fullPageHref}
						aria-label={$t('ui.sidePanel.openFull')}
						use:tooltip={{ text: $t('ui.sidePanel.openFull') }}
					>
						<ExternalLink size={18} aria-hidden="true" />
					</a>
					<!-- eslint-enable svelte/no-navigation-without-resolve -->
				{/if}
				{#if menu}
					<span bind:this={menuAnchor} class="menu-anchor">
						<IconButton
							icon={Ellipsis}
							label={$t('ui.sidePanel.more')}
							aria-haspopup="menu"
							aria-expanded={menuOpen}
							onclick={() => (menuOpen = !menuOpen)}
						/>
					</span>
				{/if}
				<IconButton
					icon={X}
					label={$t('ui.sidePanel.close')}
					onclick={() => {
						onclose();
						onfocuslist?.();
					}}
				/>
			</div>
		</header>
		<div class="body" bind:this={body}>{@render content()}</div>
		{#if footer && status === 'ready'}<div class="footer">{@render footer()}</div>{/if}
	</svelte:element>
	{#if menu}
		<Menu
			open={menuOpen}
			anchor={menuAnchor}
			onclose={() => (menuOpen = false)}
			label={$t('ui.sidePanel.more')}
			placement="bottom-end"
		>
			{@render menu()}
		</Menu>
	{/if}
{/if}

<style>
	.ui-side-panel,
	.hosted {
		display: flex;
		flex-direction: column;
		box-sizing: border-box;
		min-height: 0;
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
		animation: ui-panel-in var(--rf-duration-slow) var(--rf-ease-standard);
	}
	.ui-side-panel {
		position: relative;
		flex: none;
		height: 100%;
		background: var(--rf-bg);
	}
	.mode-docked {
		border-inline-start: var(--rf-border-width) solid var(--rf-line);
	}
	.mode-overlay {
		position: absolute;
		inset-block: 0;
		inset-inline-end: 0;
		z-index: var(--rf-z-panel);
		max-width: 100%;
		border: var(--rf-border-width) solid var(--rf-overlay-border);
		border-radius: var(--rf-radius-menu);
		background: var(--rf-overlay);
		box-shadow: var(--rf-shadow-overlay);
	}
	.mode-sheet {
		position: fixed;
		inset: 0;
		z-index: calc(var(--rf-z-scrim) + 1);
		width: auto;
		background: var(--rf-overlay);
	}
	.scrim {
		position: fixed;
		inset: 0;
		z-index: var(--rf-z-scrim);
		background: var(--rf-scrim);
	}
	.ui-side-panel:focus-visible {
		outline: none;
	}
	@keyframes ui-panel-in {
		from {
			opacity: 0;
			transform: translateX(16px);
		}
	}
	.separator {
		position: absolute;
		inset-block: 0;
		inset-inline-start: -4px;
		z-index: 1;
		width: 8px;
		cursor: col-resize;
		touch-action: none;
	}
	.separator:hover,
	.separator:focus-visible,
	.dragging .separator {
		background: linear-gradient(
			to right,
			transparent 3px,
			var(--rf-focus) 3px,
			var(--rf-focus) 5px,
			transparent 5px
		);
		outline: none;
	}
	.header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--rf-space-4);
		padding: var(--rf-space-7) var(--rf-space-8) var(--rf-space-6);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
	}
	.heading {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		min-width: 0;
	}
	.identifier {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		font-variant-numeric: tabular-nums;
	}
	.title {
		margin: 0;
		font-size: var(--rf-size-title);
		line-height: var(--rf-leading-title);
		font-weight: var(--rf-weight-semibold);
		overflow-wrap: anywhere;
	}
	.title:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 2px;
	}
	.dirty {
		display: inline-block;
		width: 6px;
		height: 6px;
		margin-inline-start: var(--rf-space-3);
		vertical-align: middle;
		border-radius: var(--rf-radius-full);
		background: var(--rf-primary);
	}
	.chip {
		margin-top: var(--rf-space-2);
	}
	.tools {
		display: flex;
		flex: none;
		gap: var(--rf-space-1);
	}
	.menu-anchor {
		display: inline-flex;
	}
	.tool {
		display: inline-grid;
		place-items: center;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		border-radius: var(--rf-radius-button);
		color: var(--rf-secondary);
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.tool:hover {
		background: var(--rf-hover);
	}
	.tool:active {
		background: var(--rf-pressed);
	}
	.tool:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		flex: 1;
		min-height: 0;
		overflow: auto;
		padding: var(--rf-space-6) var(--rf-space-8);
	}
	.footer {
		display: flex;
		justify-content: flex-end;
		gap: var(--rf-space-4);
		padding: var(--rf-space-5) var(--rf-space-8);
		border-top: var(--rf-border-width) solid var(--rf-line);
	}
	.tip {
		display: flex;
		align-items: flex-start;
		gap: var(--rf-space-4);
		padding: var(--rf-space-4) var(--rf-space-5);
		border-radius: var(--rf-radius-md);
		background: var(--rf-surface);
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
	.tip.danger {
		background: var(--rf-danger-bg);
	}
	.tip-glyph {
		display: inline-grid;
		margin-top: 1px;
		color: var(--rf-secondary);
	}
	.danger .tip-glyph {
		color: var(--rf-danger);
	}
	.tip-text {
		flex: 1;
		min-width: 0;
	}
	.tip-action {
		flex: none;
		height: 24px;
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
		cursor: pointer;
	}
	.tip-action:hover {
		background: var(--rf-hover);
	}
	.tip-action:active {
		background: var(--rf-pressed);
	}
	.tip-action:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.skeletons {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.skeleton {
		display: block;
		height: 16px;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-selected);
	}
	.skeleton:nth-child(odd) {
		width: 70%;
	}
	@media (pointer: coarse) {
		.tool {
			width: var(--rf-touch-target);
			height: var(--rf-touch-target);
		}
	}
</style>
