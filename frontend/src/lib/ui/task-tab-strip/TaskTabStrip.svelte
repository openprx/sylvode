<script module lang="ts">
	export interface StripTab {
		key: string;
		title: string;
		dirty: boolean;
		changed: boolean;
		loading?: boolean;
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { tick } from 'svelte';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import Check from '@lucide/svelte/icons/check';
	import LoaderCircle from '@lucide/svelte/icons/loader-circle';
	import X from '@lucide/svelte/icons/x';
	import Button from '../button/Button.svelte';
	import Menu from '../menu/Menu.svelte';
	import MenuItem from '../menu/MenuItem.svelte';
	import MenuSeparator from '../menu/MenuSeparator.svelte';
	import Popover from '../popover/Popover.svelte';
	import { focusTrap } from '../foundations/focus-trap';
	import { roving } from '../foundations/roving';
	import { uid } from '../foundations/ids';
	import { tooltip } from '../tooltip/tooltip';
	import { isTruncated } from '../context-line/context-line';
	import { computeOverflow, dropIndex, tabKeyAction } from './tab-strip';

	interface Props {
		tabs: StripTab[];
		active: string | null;
		/** Set while a new item needs a free tab and all eight are unsaved: shows the blocking menu. */
		pending?: { title: string } | null;
		/** Id of the panel a tab controls (aria-controls). */
		panelId: (key: string) => string;
		onselect: (key: string) => void;
		/** The parent decides whether an unsaved tab needs the discard confirmation. */
		onclose: (key: string) => void;
		onconfirmdiscard: (key: string) => void;
		/** "Keep" in the discard popover, Escape included. */
		oncanceldiscard?: () => void;
		onmove: (key: string, toIndex: number) => void;
		/** A tab was chosen in the blocking menu (key), or opening was cancelled (null). */
		onresolvepending: (key: string | null) => void;
		oncloseSaved: () => void;
		/** Optional: shows "Close other saved tabs" in the tab menu. */
		oncloseOthers?: (key: string) => void;
		/** Key whose discard popover is open. */
		confirming?: string | null;
		/** Width of the strip; measured from the element when absent. */
		width?: number;
	}

	let {
		tabs,
		active,
		pending = null,
		panelId,
		onselect,
		onclose,
		onconfirmdiscard,
		oncanceldiscard,
		onmove,
		onresolvepending,
		oncloseSaved,
		oncloseOthers,
		confirming = null,
		width
	}: Props = $props();

	const ids = {
		hint: uid('tab-hint'),
		pendingTitle: uid('tab-pending'),
		pendingBody: uid('tab-pending-body'),
		confirm: uid('tab-confirm')
	};
	let measured = $state(0);
	let tabEls = $state<Record<string, HTMLElement | null>>({});
	let overflowButton = $state<HTMLElement | null>(null);
	let overflowOpen = $state(false);
	let menuFor = $state<string | null>(null);
	let dragging = $state<string | null>(null);
	let dropTarget = $state<{ key: string; side: 'before' | 'after' } | null>(null);
	let announcement = $state('');
	let truncated = $state<Record<string, boolean>>({});
	let suppressClickUntil = 0;

	const effectiveWidth = $derived(width ?? (measured > 0 ? measured : 1000));
	const activeIndex = $derived(
		Math.max(
			0,
			tabs.findIndex((tab) => tab.key === active)
		)
	);
	const layout = $derived(computeOverflow(tabs.length, activeIndex, effectiveWidth));
	const visibleTabs = $derived(layout.visible.map((i) => tabs[i]));
	const hiddenCount = $derived(layout.hidden.length);
	const confirmTab = $derived(tabs.find((tab) => tab.key === confirming) ?? null);
	const confirmAnchor = $derived(confirming ? (tabEls[confirming] ?? overflowButton) : null);
	const menuTab = $derived(tabs.find((tab) => tab.key === menuFor) ?? null);
	const savedCount = $derived(tabs.filter((tab) => !tab.dirty).length);

	const domId = (key: string) => encodeURIComponent(key);
	const tabId = (key: string) => `task-tab-${domId(key)}`;
	const titleOf = (tab: StripTab) => tab.title;
	const nameOf = (tab: StripTab) =>
		[
			tab.title,
			tab.dirty ? $t('ui.taskTabStrip.unsaved') : null,
			tab.changed ? $t('ui.taskTabStrip.updated') : null
		]
			.filter(Boolean)
			.join(', ');

	// The blocking menu opens by itself when the parent reports that a new item has no free tab.
	let wasPending = false;
	$effect(() => {
		if (pending) overflowOpen = true;
		else if (wasPending) overflowOpen = false;
		wasPending = pending !== null;
	});

	function announceMove(key: string, toIndex: number) {
		const tab = tabs.find((x) => x.key === key);
		if (tab)
			announcement = $t('ui.taskTabStrip.moved', {
				values: { title: tab.title, position: toIndex + 1, total: tabs.length }
			});
	}

	function move(key: string, toIndex: number) {
		const from = tabs.findIndex((tab) => tab.key === key);
		if (from < 0 || from === toIndex) return;
		onmove(key, toIndex);
		announceMove(key, toIndex);
		void tick().then(() => tabEls[key]?.focus());
	}

	function onTabKeydown(event: KeyboardEvent, key: string) {
		if (confirming) return;
		const index = tabs.findIndex((tab) => tab.key === key);
		const action = tabKeyAction(event, index, tabs.length);
		if (!action) return;
		event.preventDefault();
		if (action.kind === 'close') onclose(key);
		else if (action.kind === 'move') move(key, action.toIndex);
		else if (action.kind === 'menu') menuFor = key;
		else {
			const next = tabs[action.index];
			if (!next) return;
			// The next tab may be hidden in the overflow menu; it is then shown by activating it.
			tabEls[next.key]?.focus();
			select(next.key);
			void tick().then(() => tabEls[next.key]?.focus());
		}
	}

	function select(key: string) {
		if (dragging || Date.now() < suppressClickUntil) return;
		onselect(key);
	}

	function noteTruncation(key: string, el: HTMLElement) {
		const label = el.querySelector<HTMLElement>('.label');
		truncated[key] = label ? isTruncated(label) : false;
	}

	function dragStart(event: DragEvent, key: string) {
		if (confirming) {
			event.preventDefault();
			return;
		}
		dragging = key;
		dropTarget = null;
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
			event.dataTransfer.setData('application/x-sv-task-tab', key);
		}
	}
	function dragOver(event: DragEvent, key: string) {
		if (!dragging || dragging === key) return;
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
		const bounds = (event.currentTarget as HTMLElement).getBoundingClientRect();
		dropTarget = { key, side: event.clientX < bounds.left + bounds.width / 2 ? 'before' : 'after' };
	}
	function finishDrag() {
		dragging = null;
		dropTarget = null;
		suppressClickUntil = Date.now() + 250;
	}
	function drop(event: DragEvent, key: string) {
		if (!dragging) return;
		event.preventDefault();
		event.stopPropagation();
		const source = dragging;
		const target = dropTarget?.key === key ? dropTarget : null;
		const to = target
			? dropIndex(
					tabs.map((tab) => tab.key),
					source,
					target.key,
					target.side
				)
			: null;
		if (to !== null) {
			onmove(source, to);
			announceMove(source, to);
		}
		finishDrag();
	}

	function closeOverflow() {
		overflowOpen = false;
		if (pending) onresolvepending(null);
	}
	function chooseFromOverflow(key: string) {
		overflowOpen = false;
		if (pending) onresolvepending(key);
		else onselect(key);
	}
</script>

<p id={ids.hint} class="sr-only">{$t('ui.taskTabStrip.reorderHint')}</p>
<div class="sr-only" role="status" aria-live="polite" data-testid="task-tab-reorder-status">
	{announcement}
</div>

{#if tabs.length}
	<div
		class="ui-tab-strip"
		bind:clientWidth={measured}
		data-testid="task-workbench"
		data-ui="task-tab-strip"
	>
		<div
			class="tablist"
			role="tablist"
			tabindex="-1"
			aria-label={$t('ui.taskTabStrip.openTasks')}
			aria-describedby={ids.hint}
			data-testid="task-tablist"
			data-mode={layout.mode}
		>
			{#each visibleTabs as tab (tab.key)}
				{@const isActive = active === tab.key}
				<div
					class="slot"
					class:active={isActive}
					class:dragging={dragging === tab.key}
					style:width="{layout.tabWidth}px"
					role="presentation"
					data-testid="task-tab-drop-target"
					data-tab-key={tab.key}
					ondragover={(event) => dragOver(event, tab.key)}
					ondrop={(event) => drop(event, tab.key)}
				>
					{#if dropTarget?.key === tab.key}
						<span
							class="drop-line"
							data-side={dropTarget.side}
							aria-hidden="true"
							data-testid="task-tab-drop-indicator"
							data-drop-side={dropTarget.side}
						></span>
					{/if}
					<button
						bind:this={tabEls[tab.key]}
						type="button"
						role="tab"
						class="tab"
						id={tabId(tab.key)}
						aria-selected={isActive}
						aria-controls={panelId(tab.key)}
						aria-label={nameOf(tab)}
						aria-keyshortcuts="Alt+Shift+ArrowLeft Alt+Shift+ArrowRight"
						tabindex={isActive ? 0 : -1}
						draggable="true"
						data-testid="task-tab"
						data-tab-key={tab.key}
						data-dragging={dragging === tab.key}
						use:tooltip={{ text: truncated[tab.key] ? tab.title : '' }}
						onpointerenter={(e) => noteTruncation(tab.key, e.currentTarget)}
						onfocus={(e) => noteTruncation(tab.key, e.currentTarget)}
						onclick={() => select(tab.key)}
						onkeydown={(event) => onTabKeydown(event, tab.key)}
						oncontextmenu={(event) => {
							event.preventDefault();
							menuFor = tab.key;
						}}
						onmousedown={(event) => event.button === 1 && event.preventDefault()}
						onauxclick={(event) => event.button === 1 && onclose(tab.key)}
						ondragstart={(event) => dragStart(event, tab.key)}
						ondragend={finishDrag}
					>
						{#if tab.loading}<LoaderCircle size={14} class="spin" aria-hidden="true" />{/if}
						<span class="label">{titleOf(tab)}</span>
						{#if tab.dirty}<span class="dot" aria-hidden="true"></span>{/if}
						{#if tab.changed}<span class="ring" aria-hidden="true"></span>{/if}
					</button>
					<button
						type="button"
						class="close"
						tabindex="-1"
						draggable="false"
						aria-label={$t('ui.taskTabStrip.close', { values: { title: tab.title } })}
						data-testid="task-tab-close"
						data-tab-key={tab.key}
						onclick={() => {
							if (!dragging && Date.now() >= suppressClickUntil) onclose(tab.key);
						}}
					>
						<X size={14} aria-hidden="true" />
					</button>
				</div>
			{/each}
		</div>
		<button
			bind:this={overflowButton}
			type="button"
			class="overflow"
			aria-haspopup="menu"
			aria-expanded={overflowOpen}
			aria-label={$t('ui.taskTabStrip.allTabs', { values: { count: tabs.length } })}
			data-testid="task-tab-overflow"
			onclick={() => (overflowOpen = !overflowOpen)}
			onkeydown={(event) => {
				if (event.key === 'ArrowDown') {
					event.preventDefault();
					overflowOpen = true;
				}
			}}
		>
			{#if layout.mode === 'collapsed'}
				<span>{$t('ui.taskTabStrip.tabCount', { values: { count: tabs.length } })}</span>
			{:else if hiddenCount > 0}
				<span>+{hiddenCount}</span>
			{/if}
			<ChevronDown size={14} aria-hidden="true" />
		</button>
	</div>
{/if}

<!-- Overflow menu, and the blocking "close one to continue" variant of it. -->
<Popover
	open={overflowOpen}
	anchor={overflowButton}
	onclose={closeOverflow}
	role="menu"
	labelledby={pending ? ids.pendingTitle : undefined}
	label={pending ? undefined : $t('ui.taskTabStrip.openTasks')}
	placement="bottom-end"
	width={300}
	padding="menu"
	initialFocus="[role='menuitemradio'], [role='menuitem']"
>
	<div
		class="items"
		data-testid={pending ? 'task-tab-limit' : 'task-tab-overflow-menu'}
		aria-describedby={pending ? ids.pendingBody : undefined}
		use:roving={{ selector: '[role="menuitemradio"], [role="menuitem"]', orientation: 'vertical' }}
	>
		{#if pending}
			<div class="caption strong" id={ids.pendingTitle}>{$t('ui.taskTabStrip.closeOneTitle')}</div>
			<div class="caption" id={ids.pendingBody}>
				{$t('ui.taskTabStrip.closeOneBody', {
					values: { count: tabs.length, title: pending.title }
				})}
			</div>
		{:else}
			<div class="caption">
				{$t('ui.taskTabStrip.openTabs', { values: { count: tabs.length } })}
			</div>
		{/if}
		{#each tabs as tab (tab.key)}
			<button
				type="button"
				role="menuitemradio"
				aria-checked={active === tab.key}
				tabindex="-1"
				class="entry"
				data-tab-key={tab.key}
				data-testid="task-tab-overflow-item"
				onclick={() => chooseFromOverflow(tab.key)}
			>
				<span class="check"
					>{#if active === tab.key}<Check size={16} aria-hidden="true" />{/if}</span
				>
				<span class="name">{tab.title}</span>
				{#if tab.dirty}<span class="dot" role="img" aria-label={$t('ui.taskTabStrip.unsavedShort')}
					></span>{/if}
			</button>
		{/each}
		<div class="rule" role="separator"></div>
		{#if pending}
			<button
				type="button"
				role="menuitem"
				tabindex="-1"
				class="entry"
				data-testid="task-tab-cancel-opening"
				onclick={closeOverflow}
			>
				<span class="check"></span>
				<span class="name">{$t('ui.taskTabStrip.cancelOpening')}</span>
				<span class="hint">{$t('ui.taskTabStrip.escape')}</span>
			</button>
		{:else}
			<button
				type="button"
				role="menuitem"
				tabindex="-1"
				class="entry"
				aria-disabled={savedCount === 0 || undefined}
				onclick={() => {
					if (savedCount === 0) return;
					overflowOpen = false;
					oncloseSaved();
				}}
			>
				<span class="check"></span>
				<span class="name">{$t('ui.taskTabStrip.closeSaved')}</span>
			</button>
		{/if}
	</div>
</Popover>

<!-- Tab context menu: right-click, Shift+F10 or the menu key. -->
<Menu
	open={menuTab !== null}
	anchor={menuTab ? (tabEls[menuTab.key] ?? null) : null}
	onclose={() => (menuFor = null)}
	label={$t('ui.taskTabStrip.tabMenu')}
	width={220}
>
	{#if menuTab}
		{@const key = menuTab.key}
		{@const index = tabs.findIndex((tab) => tab.key === key)}
		<MenuItem compact onselect={() => onclose(key)}>{$t('ui.taskTabStrip.closeTab')}</MenuItem>
		{#if oncloseOthers}
			<MenuItem compact onselect={() => oncloseOthers(key)}
				>{$t('ui.taskTabStrip.closeOthers')}</MenuItem
			>
		{/if}
		<MenuSeparator />
		<MenuItem compact disabled={index <= 0} onselect={() => move(key, index - 1)}>
			{$t('ui.taskTabStrip.moveLeft')}
		</MenuItem>
		<MenuItem compact disabled={index >= tabs.length - 1} onselect={() => move(key, index + 1)}>
			{$t('ui.taskTabStrip.moveRight')}
		</MenuItem>
	{/if}
</Menu>

<!-- Discard confirmation bound to one tab: a small destructive dialog in a popover. -->
<Popover
	open={confirmTab !== null}
	anchor={confirmAnchor}
	onclose={() => oncanceldiscard?.()}
	role="none"
	width={280}
	placement="bottom-start"
	initialFocus="[data-autofocus]"
>
	{#if confirmTab}
		<div
			class="confirm"
			role="dialog"
			aria-labelledby={ids.confirm}
			data-testid="task-close-confirm"
			use:focusTrap={{ initialFocus: '[data-autofocus]' }}
		>
			<h2 class="question" id={ids.confirm}>
				{$t('ui.taskTabStrip.discardTitle', { values: { title: confirmTab.title } })}
			</h2>
			<p class="body">{$t('ui.taskTabStrip.discardBody')}</p>
			<div class="actions">
				<Button data-autofocus onclick={() => oncanceldiscard?.()}
					>{$t('ui.taskTabStrip.keep')}</Button
				>
				<Button variant="danger" onclick={() => onconfirmdiscard(confirmTab.key)}>
					{$t('ui.taskTabStrip.discardClose')}
				</Button>
			</div>
		</div>
	{/if}
</Popover>

<style>
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		margin: -1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	.ui-tab-strip {
		display: flex;
		align-items: center;
		gap: var(--rf-space-1);
		box-sizing: border-box;
		height: 40px;
		padding: 0 var(--rf-space-4);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		background: var(--rf-nav);
		font-family: var(--rf-font-sans-stack);
	}
	.tablist {
		display: flex;
		align-items: center;
		gap: var(--rf-space-1);
		min-width: 0;
		flex: 1 1 auto;
	}
	.slot {
		position: relative;
		display: flex;
		align-items: center;
		box-sizing: border-box;
		flex: none;
		height: 32px;
		padding-inline-end: var(--rf-space-2);
		border-radius: var(--rf-radius-sm);
		transition: background-color var(--rf-duration-fast) var(--rf-ease-standard);
	}
	.slot:hover:not(.active) {
		background: var(--rf-hover-nav);
	}
	.slot.active {
		background: var(--rf-bg);
		box-shadow: inset 0 0 0 var(--rf-border-width) var(--rf-line);
	}
	.slot.dragging {
		background: var(--rf-selected);
	}
	.tab {
		position: relative;
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		flex: 1 1 auto;
		min-width: 0;
		height: 100%;
		padding: 0 0 0 var(--rf-space-5);
		border: 0;
		border-radius: var(--rf-radius-sm);
		background: transparent;
		color: var(--rf-secondary);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-regular);
		text-align: start;
		cursor: pointer;
	}
	.slot:hover .tab,
	.slot.active .tab {
		color: var(--rf-text);
	}
	.tab:active {
		background: var(--rf-pressed);
	}
	.tab:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	/* Proposed (batch A): 2px primary bar marks the active tab; fill and outline alone are not a usable marker. */
	.slot.active .tab::after {
		content: '';
		position: absolute;
		inset-inline: var(--rf-space-5);
		bottom: 0;
		height: 2px;
		border-radius: 2px;
		background: var(--rf-primary);
	}
	.label {
		flex: 0 1 auto;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.dot {
		flex: none;
		width: 6px;
		height: 6px;
		border-radius: var(--rf-radius-full);
		background: var(--rf-primary);
	}
	.ring {
		flex: none;
		box-sizing: border-box;
		width: 6px;
		height: 6px;
		border: 1.5px solid var(--rf-primary);
		border-radius: var(--rf-radius-full);
	}
	.tab :global(.spin) {
		flex: none;
		animation: ui-tab-spin 900ms linear infinite;
	}
	@keyframes ui-tab-spin {
		to {
			transform: rotate(360deg);
		}
	}
	.close {
		flex: none;
		display: inline-grid;
		place-items: center;
		width: 24px;
		height: 24px;
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
	}
	.close:hover {
		background: var(--rf-hover-nav);
		color: var(--rf-text);
	}
	.slot.active .close:hover {
		background: var(--rf-hover);
	}
	.drop-line {
		position: absolute;
		top: 4px;
		width: 2px;
		height: 24px;
		border-radius: 2px;
		background: var(--rf-primary);
		pointer-events: none;
	}
	.drop-line[data-side='before'] {
		inset-inline-start: -2px;
	}
	.drop-line[data-side='after'] {
		inset-inline-end: -2px;
	}
	.overflow {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-2);
		flex: none;
		height: 32px;
		padding: 0 var(--rf-space-4);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		cursor: pointer;
	}
	.overflow:hover {
		background: var(--rf-hover-nav);
	}
	.overflow:active,
	.overflow[aria-expanded='true'] {
		background: var(--rf-pressed);
	}
	.overflow:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.items {
		display: flex;
		flex-direction: column;
	}
	.caption {
		padding: var(--rf-space-3) var(--rf-space-5);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
	}
	.caption.strong {
		padding-bottom: 0;
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
	}
	.entry {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		width: 100%;
		min-height: 32px;
		padding: 0 var(--rf-space-5);
		border: 0;
		border-radius: var(--rf-radius-sm);
		background: transparent;
		color: var(--rf-text);
		font-family: inherit;
		font-size: var(--rf-size-secondary);
		text-align: start;
		cursor: pointer;
	}
	.entry:hover:not([aria-disabled='true']) {
		background: var(--rf-surface);
	}
	.entry:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.entry[aria-disabled='true'] {
		color: var(--rf-control);
		cursor: not-allowed;
	}
	.check {
		display: inline-grid;
		flex: none;
		width: 16px;
	}
	.name {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.hint {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.rule {
		height: 1px;
		margin: var(--rf-space-2) 0;
		background: var(--rf-line);
	}
	.confirm {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-2);
	}
	.question {
		margin: 0;
		color: var(--rf-text);
		font-size: var(--rf-size-button);
		font-weight: var(--rf-weight-semibold);
		line-height: var(--rf-leading-button);
	}
	.body {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
	.actions {
		display: flex;
		justify-content: flex-end;
		gap: var(--rf-space-4);
	}
	@media (pointer: coarse) {
		.close {
			position: relative;
		}
		.close::after {
			content: '';
			position: absolute;
			inset: -8px -10px;
		}
		.entry {
			min-height: var(--rf-touch-target);
		}
	}
</style>
