<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { ShellState } from '../shell-state.svelte';
	import { homeRows, recentRows, type Translate } from './lists';
	import NavList from './NavList.svelte';
	import { selectedRowId, type NavRow } from './rows';
	import type { RouteUrl } from './types';

	interface Props {
		shell: ShellState;
		url: RouteUrl;
		unread: number;
		onfollow?: (row?: NavRow) => void;
	}
	let { shell, url, unread, onfollow }: Props = $props();

	const tr: Translate = (key, values) => $t(key, values ? { values } : undefined);
	const main = $derived(homeRows({ unread, tr }));
	const recent = $derived(recentRows(shell.recent));
	const selectedId = $derived(selectedRowId([...main, ...recent], url));
</script>

<div class="group" data-column-group="home">
	<NavList rows={main} {selectedId} {onfollow} />
</div>
{#if recent.length > 0}
	<div class="group" data-column-group="recent">
		<p class="caption">{$t('shell.column.home.recent')}</p>
		<NavList rows={recent} {selectedId} {onfollow} />
	</div>
{/if}

<style>
	.group {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
	}
	.caption {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-4) var(--rf-space-2);
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
		line-height: var(--rf-leading-meta);
		font-weight: var(--rf-weight-medium);
	}
</style>
