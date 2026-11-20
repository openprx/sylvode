<script lang="ts">
	import { t } from 'svelte-i18n';
	import Menu from '../../ui/menu/Menu.svelte';
	import MenuItem from '../../ui/menu/MenuItem.svelte';
	import MenuSeparator from '../../ui/menu/MenuSeparator.svelte';
	import {
		LOCALE_OPTIONS,
		PERSONAL_SETTINGS_HREF,
		THEME_OPTIONS,
		type AccountControls
	} from './account-model';

	interface Props extends AccountControls {
		open: boolean;
		/** The rail's account button: gets focus back on close. */
		anchor: HTMLElement | null;
		onclose: () => void;
	}

	let {
		open,
		anchor,
		onclose,
		user,
		theme,
		locale,
		ontheme,
		onlocale,
		onsignout,
		settingsHref = PERSONAL_SETTINGS_HREF
	}: Props = $props();
</script>

<Menu
	{open}
	{anchor}
	{onclose}
	label={$t('shell.account.label')}
	width={240}
	placement="top-start"
	data-shell="account-menu"
>
	{#snippet header()}
		<div class="who" data-shell="account-who">
			<span class="name">{user.name}</span>
			<span class="email">{user.email}</span>
		</div>
	{/snippet}

	<div role="group" aria-label={$t('shell.account.theme')} class="group">
		<span class="caption" aria-hidden="true">{$t('shell.account.theme')}</span>
		{#each THEME_OPTIONS as option (option.value)}
			<MenuItem
				role="menuitemradio"
				checked={theme === option.value}
				keepOpen
				data-theme-option={option.value}
				onselect={() => ontheme(option.value)}
			>
				{$t(option.labelKey)}
			</MenuItem>
		{/each}
	</div>

	<div role="group" aria-label={$t('shell.account.language')} class="group">
		<span class="caption" aria-hidden="true">{$t('shell.account.language')}</span>
		{#each LOCALE_OPTIONS as option (option.value)}
			<MenuItem
				role="menuitemradio"
				checked={locale === option.value}
				keepOpen
				data-locale-option={option.value}
				onselect={() => onlocale(option.value)}
			>
				{option.label}
			</MenuItem>
		{/each}
	</div>

	<MenuSeparator />
	<MenuItem href={settingsHref} data-shell="account-settings">
		{$t('shell.account.personalSettings')}
	</MenuItem>
	<MenuItem data-shell="account-signout" onselect={onsignout}>
		{$t('shell.account.signOut')}
	</MenuItem>
</Menu>

<style>
	.who {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-1);
		padding: var(--rf-space-4) var(--rf-space-5);
		border-block-end: var(--rf-border-width) solid var(--rf-line);
		font-family: var(--rf-font-sans-stack);
	}
	.name {
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
		overflow-wrap: anywhere;
	}
	.email {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		overflow-wrap: anywhere;
	}
	.group {
		display: flex;
		flex-direction: column;
	}
	.caption {
		padding: var(--rf-space-4) var(--rf-space-5) var(--rf-space-2);
		color: var(--rf-secondary);
		font-family: var(--rf-font-sans-stack);
		font-size: var(--rf-size-meta);
		font-weight: var(--rf-weight-medium);
	}
</style>
