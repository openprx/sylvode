<script lang="ts">
	import { t } from 'svelte-i18n';
	import PencilLine from '@lucide/svelte/icons/pencil-line';
	import IconButton from '../../ui/icon-button/IconButton.svelte';
	import Avatar from '../../ui/avatar/Avatar.svelte';
	import { tooltip } from '../../ui/tooltip/tooltip';
	import { nextRovingIndex } from '../../ui/foundations/roving';
	import AccountMenu from '../account/AccountMenu.svelte';
	import type { AccountControls } from '../account/account-model';
	import BrandMark from '../brand/BrandMark.svelte';
	import type { Destination } from '../route-map';
	import type { RailShortcuts, Shortcut } from '../shortcuts.svelte';
	import RailItem from './RailItem.svelte';
	import ShortcutEditor from './ShortcutEditor.svelte';
	import { glyphFor } from './glyphs';
	import { isToggleTarget, type RailItemModel, type RailModel } from './rail-model';

	interface Props {
		model: RailModel;
		/** Whether the selected destination's column is open: sets `aria-expanded` on its item. */
		columnExpanded: boolean;
		/** Activating the already selected destination asks the shell to toggle its column. */
		ontogglecolumn: (destination: Destination) => void;
		account: AccountControls;
		shortcuts: RailShortcuts;
		/** The page "Add" in the editor would pin; null when it cannot be pinned. */
		currentPage: Shortcut | null;
		/** Neutral confirmation for "Restore default navigation". */
		confirmRestore: () => Promise<boolean>;
	}

	let {
		model,
		columnExpanded,
		ontogglecolumn,
		account,
		shortcuts,
		currentPage,
		confirmRestore
	}: Props = $props();

	const SettingsGlyph = glyphFor('settings');
	let group = $state<HTMLElement | null>(null);
	let editorOpen = $state(false);
	let editorButton = $state<HTMLElement | null>(null);
	let accountOpen = $state(false);
	let accountButton = $state<HTMLElement | null>(null);

	/** One roving stop for destinations and shortcuts: the focused item, else the selected one, else the first. */
	const all = $derived<RailItemModel[]>([...model.destinations, ...model.shortcuts]);
	let focusedId = $state<string | null>(null);
	const stopId = $derived(
		(focusedId && all.some((i) => i.id === focusedId) ? focusedId : null) ??
			all.find((i) => i.selected)?.id ??
			all[0]?.id ??
			null
	);

	function itemElements(): HTMLElement[] {
		return group
			? [...group.querySelectorAll<HTMLElement>('[data-rail-destination], [data-rail-shortcut]')]
			: [];
	}

	function onkeydown(event: KeyboardEvent) {
		if (event.altKey || event.ctrlKey || event.metaKey) return;
		const els = itemElements();
		const current = els.indexOf(document.activeElement as HTMLElement);
		if (current < 0) return;
		const next = nextRovingIndex(
			event.key,
			current,
			els.map(() => true),
			{ orientation: 'vertical' }
		);
		if (next === null) return;
		event.preventDefault();
		els[next].focus();
	}

	function onfocusin(event: FocusEvent) {
		const el = event.target as HTMLElement;
		focusedId = el.dataset.railDestination ?? el.dataset.railShortcut ?? focusedId;
	}

	function activate(item: RailItemModel, event: MouseEvent) {
		if (!isToggleTarget(item)) return;
		event.preventDefault();
		ontogglecolumn(item.id as Destination);
	}
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- hrefs are full in-app paths built by the shell model -->

<nav class="rail" aria-label={$t('shell.rail.label')} data-shell="rail" data-layout="rail">
	<a
		class="brand"
		href={model.homeHref}
		aria-label={$t('shell.brand.home')}
		use:tooltip={{ text: $t('shell.brand.home') }}
	>
		<BrandMark decorative />
	</a>

	<!-- Arrow keys move between destinations and shortcuts (one Tab stop). -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div
		class="group"
		role="group"
		aria-label={$t('shell.rail.destinations')}
		bind:this={group}
		{onkeydown}
		{onfocusin}
	>
		{#each model.destinations as item (item.id)}
			<RailItem
				{item}
				tabindex={item.id === stopId ? 0 : -1}
				columnExpanded={item.selected ? columnExpanded : undefined}
				onactivate={activate}
			/>
		{/each}
		{#if model.shortcuts.length > 0}
			<div class="divider" role="separator" data-shell="rail-divider"></div>
			<div class="shortcuts" data-shell="rail-shortcuts">
				{#each model.shortcuts as item (item.id)}
					<RailItem {item} tabindex={item.id === stopId ? 0 : -1} onactivate={activate} />
				{/each}
			</div>
		{/if}
	</div>

	<div class="bottom">
		<span bind:this={editorButton} class="tool">
			<IconButton
				icon={PencilLine}
				label={$t('shell.rail.editNavigation')}
				data-shell="edit-navigation"
				aria-haspopup="dialog"
				aria-expanded={editorOpen}
				onclick={() => (editorOpen = !editorOpen)}
			/>
		</span>
		<a
			class="settings"
			href={model.settings.href}
			data-shell="settings"
			aria-current={model.settings.selected ? 'page' : undefined}
		>
			<SettingsGlyph size={20} aria-hidden="true" />
			<span class="label">{model.settings.label}</span>
		</a>
		<button
			bind:this={accountButton}
			type="button"
			class="account"
			data-shell="account"
			aria-haspopup="menu"
			aria-expanded={accountOpen}
			aria-label={$t('shell.rail.account', { values: { name: account.user.name } })}
			onclick={() => (accountOpen = !accountOpen)}
		>
			<Avatar name={account.user.name} size="md" decorative />
		</button>
	</div>
</nav>

<ShortcutEditor
	open={editorOpen}
	anchor={editorButton}
	onclose={() => (editorOpen = false)}
	{shortcuts}
	destinations={model.destinations}
	current={currentPage}
	{confirmRestore}
/>
<AccountMenu
	open={accountOpen}
	anchor={accountButton}
	onclose={() => (accountOpen = false)}
	{...account}
/>

<style>
	.rail {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-5);
		box-sizing: border-box;
		width: var(--rf-layout-rail);
		height: 100%;
		padding: var(--rf-space-7) var(--rf-space-4);
		background: var(--rf-rail);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
	.brand {
		display: grid;
		place-items: center;
		width: 52px;
		height: 36px;
		flex: none;
		border-radius: var(--rf-radius-sm);
		color: var(--rf-text);
	}
	.brand:focus-visible,
	.settings:focus-visible,
	.account:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.group {
		display: flex;
		flex: 1;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-5);
		min-height: 0;
		box-sizing: border-box;
		/* The scrolling shortcut list would clip the selection bar that sits in the padding. */
		width: calc(100% + 2 * var(--rf-space-4));
		padding-inline: var(--rf-space-4);
	}
	.divider {
		flex: none;
		width: 28px;
		height: var(--rf-border-width);
		background: var(--rf-line);
	}
	.shortcuts {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-5);
		width: 100%;
		min-height: 0;
		overflow-y: auto;
		scrollbar-width: none;
	}
	.bottom {
		display: flex;
		flex: none;
		flex-direction: column;
		align-items: center;
		gap: var(--rf-space-5);
	}
	.tool {
		display: inline-flex;
	}
	.settings {
		position: relative;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: var(--rf-space-2);
		box-sizing: border-box;
		width: 52px;
		height: 52px;
		border-radius: var(--rf-radius-sm);
		color: var(--rf-text);
		text-decoration: none;
	}
	.settings:hover {
		background: var(--rf-hover-rail);
	}
	.settings[aria-current='page'] {
		background: var(--rf-selected);
	}
	.settings[aria-current='page']::before {
		content: '';
		position: absolute;
		inset-inline-start: calc(-1 * var(--rf-space-4));
		top: 50%;
		width: var(--rf-border-width-focus);
		height: 26px;
		transform: translateY(-50%);
		background: var(--rf-primary);
	}
	.settings .label {
		font-size: var(--rf-size-caption);
		font-weight: var(--rf-weight-medium);
		line-height: 1.2;
	}
	.account {
		display: grid;
		place-items: center;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-full);
		background: transparent;
		cursor: pointer;
	}
</style>
