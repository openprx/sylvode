<script lang="ts">
	import { t } from 'svelte-i18n';
	import Ellipsis from '@lucide/svelte/icons/ellipsis';
	import Avatar from '../../ui/avatar/Avatar.svelte';
	import IconButton from '../../ui/icon-button/IconButton.svelte';
	import Menu from '../../ui/menu/Menu.svelte';
	import MenuItem from '../../ui/menu/MenuItem.svelte';
	import MenuSeparator from '../../ui/menu/MenuSeparator.svelte';
	import { nextRovingIndex } from '../../ui/foundations/roving';
	import AccountMenu from '../account/AccountMenu.svelte';
	import type { AccountControls } from '../account/account-model';
	import BrandMark from '../brand/BrandMark.svelte';
	import type { Destination } from '../route-map';
	import type { RailShortcuts, Shortcut } from '../shortcuts.svelte';
	import RailItem from './RailItem.svelte';
	import ShortcutEditor from './ShortcutEditor.svelte';
	import type { RailItemModel, RailModel } from './rail-model';

	interface Props {
		model: RailModel;
		/** A destination was activated: the shell shows its column full width, without navigating. */
		ondestination: (destination: Destination) => void;
		account: AccountControls;
		shortcuts: RailShortcuts;
		currentPage: Shortcut | null;
		confirmRestore: () => Promise<boolean>;
	}

	let { model, ondestination, account, shortcuts, currentPage, confirmRestore }: Props = $props();

	let list = $state<HTMLElement | null>(null);
	let moreOpen = $state(false);
	let moreButton = $state<HTMLElement | null>(null);
	let editorOpen = $state(false);
	let accountOpen = $state(false);
	let accountButton = $state<HTMLElement | null>(null);
	let focusedId = $state<string | null>(null);

	const stopId = $derived(
		(focusedId && model.destinations.some((i) => i.id === focusedId) ? focusedId : null) ??
			model.destinations.find((i) => i.selected)?.id ??
			model.destinations[0]?.id ??
			null
	);
	const selectedId = $derived(model.destinations.find((i) => i.selected)?.id ?? null);

	// The four labels may not fit: keep the selected one in view.
	$effect(() => {
		if (!list || !selectedId) return;
		list
			.querySelector<HTMLElement>(`[data-rail-destination="${selectedId}"]`)
			?.scrollIntoView?.({ inline: 'nearest', block: 'nearest' });
	});

	function onkeydown(event: KeyboardEvent) {
		if (event.altKey || event.ctrlKey || event.metaKey || !list) return;
		const els = [...list.querySelectorAll<HTMLElement>('[data-rail-destination]')];
		const current = els.indexOf(document.activeElement as HTMLElement);
		if (current < 0) return;
		const next = nextRovingIndex(
			event.key,
			current,
			els.map(() => true),
			{ orientation: 'horizontal' }
		);
		if (next === null) return;
		event.preventDefault();
		els[next].focus();
	}

	function onfocusin(event: FocusEvent) {
		focusedId = (event.target as HTMLElement).dataset.railDestination ?? focusedId;
	}

	function activate(item: RailItemModel, event: MouseEvent) {
		event.preventDefault();
		ondestination(item.id as Destination);
	}
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- hrefs are full in-app paths built by the shell model -->

<nav class="topbar" aria-label={$t('shell.rail.label')} data-shell="rail" data-layout="topbar">
	<a class="brand" href={model.homeHref} aria-label={$t('shell.brand.home')}>
		<BrandMark decorative size={24} />
	</a>

	<!-- Arrow keys move between the destinations (one Tab stop). -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div
		class="destinations"
		role="group"
		aria-label={$t('shell.rail.destinations')}
		bind:this={list}
		{onkeydown}
		{onfocusin}
	>
		{#each model.destinations as item (item.id)}
			<RailItem
				{item}
				layout="topbar"
				tabindex={item.id === stopId ? 0 : -1}
				onactivate={activate}
			/>
		{/each}
	</div>

	<span bind:this={moreButton} class="tool">
		<IconButton
			icon={Ellipsis}
			label={$t('shell.rail.more')}
			data-shell="rail-more"
			aria-haspopup="menu"
			aria-expanded={moreOpen}
			onclick={() => (moreOpen = !moreOpen)}
		/>
	</span>
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
</nav>

<Menu
	open={moreOpen}
	anchor={moreButton}
	onclose={() => (moreOpen = false)}
	label={$t('shell.rail.more')}
	placement="bottom-end"
	data-shell="rail-more-menu"
>
	{#each model.shortcuts as item (item.id)}
		<MenuItem
			href={item.href ?? undefined}
			disabled={item.href === null}
			data-rail-shortcut={item.id}
			data-shortcut-kind={item.shortcutKind}
		>
			{item.label}
		</MenuItem>
	{/each}
	{#if model.shortcuts.length > 0}<MenuSeparator />{/if}
	<MenuItem onselect={() => (editorOpen = true)} data-shell="edit-navigation">
		{$t('shell.rail.editNavigation')}
	</MenuItem>
	<MenuItem href={model.settings.href} data-shell="settings">{model.settings.label}</MenuItem>
</Menu>

<ShortcutEditor
	open={editorOpen}
	anchor={moreButton}
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
	.topbar {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		box-sizing: border-box;
		width: 100%;
		height: 64px;
		padding: 0 var(--rf-space-6);
		background: var(--rf-rail);
		border-block-end: var(--rf-border-width) solid var(--rf-line);
		color: var(--rf-text);
		font-family: var(--rf-font-sans-stack);
	}
	.brand {
		display: grid;
		place-items: center;
		flex: none;
		width: 36px;
		height: 36px;
		border-radius: var(--rf-radius-sm);
		color: var(--rf-text);
	}
	.brand:focus-visible,
	.account:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.destinations {
		display: flex;
		flex: 1;
		min-width: 0;
		gap: var(--rf-space-2);
		overflow-x: auto;
		scrollbar-width: none;
	}
	.tool {
		display: inline-flex;
		flex: none;
	}
	.account {
		display: grid;
		flex: none;
		place-items: center;
		width: var(--rf-control-height);
		height: var(--rf-control-height);
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-full);
		background: transparent;
		cursor: pointer;
	}
	@media (max-width: 479px) {
		.brand {
			display: none;
		}
	}
</style>
