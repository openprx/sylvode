<script lang="ts">
	import { locale, t } from 'svelte-i18n';
	import Menu from '#lib/ui/menu/Menu.svelte';
	import MenuItem from '#lib/ui/menu/MenuItem.svelte';
	import MenuSeparator from '#lib/ui/menu/MenuSeparator.svelte';
	import { PERSONAL_SCOPE, type ScopeKey } from '../destinations';
	import type { Load } from './column-data.svelte';
	import { sortWorkspaces, type WorkspaceRef } from './scope';

	interface Props {
		open: boolean;
		anchor: HTMLElement | null;
		onclose: () => void;
		scope: ScopeKey;
		workspaces: Load<WorkspaceRef>;
		onselect: (scope: ScopeKey) => void;
		onretry: () => void;
		/** Menu width in px. */
		width?: number;
	}

	let {
		open,
		anchor,
		onclose,
		scope,
		workspaces,
		onselect,
		onretry,
		width = 240
	}: Props = $props();

	const sorted = $derived(sortWorkspaces(workspaces.items, $locale ?? undefined));
</script>

<Menu
	{open}
	{anchor}
	{onclose}
	{width}
	label={$t('shell.column.scope.menuLabel')}
	testId="scope-menu"
>
	<MenuItem
		role="menuitemradio"
		checked={scope === PERSONAL_SCOPE}
		onselect={() => onselect(PERSONAL_SCOPE)}
		data-scope-option={PERSONAL_SCOPE}
	>
		{$t('shell.column.scope.personal')}
	</MenuItem>
	{#each sorted as workspace (workspace.id)}
		<MenuItem
			role="menuitemradio"
			checked={scope === workspace.id}
			onselect={() => onselect(workspace.id)}
			data-scope-option={workspace.id}
		>
			{workspace.name}
		</MenuItem>
	{/each}
	{#if workspaces.status === 'loading' || workspaces.status === 'idle'}
		<MenuItem disabled>{$t('shell.column.scope.loading')}</MenuItem>
	{:else if workspaces.status === 'error' || workspaces.status === 'forbidden'}
		<MenuItem keepOpen onselect={onretry}>{$t('shell.column.scope.retry')}</MenuItem>
	{/if}
	<MenuSeparator />
	<MenuItem href="/workspace">{$t('shell.column.scope.all')}</MenuItem>
</Menu>
