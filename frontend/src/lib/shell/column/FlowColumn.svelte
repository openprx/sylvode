<script lang="ts">
	import { t } from 'svelte-i18n';
	import { projectOptionsStore as appOptionsStore } from '#lib/stores/project-options.js';
	import type { ColumnData } from './column-data.svelte';
	import type { ScopeKey } from '../destinations';
	import { flowList, type Translate } from './lists';
	import NavList from './NavList.svelte';
	import { selectedRowId, type NavRow } from './rows';
	import SkeletonRows from './SkeletonRows.svelte';
	import type { ColumnViewer, ProjectOptionsSource, RouteUrl } from './types';

	interface Props {
		scope: ScopeKey;
		url: RouteUrl;
		viewer: ColumnViewer;
		data: ColumnData;
		onfollow?: (row?: NavRow) => void;
		/** Project option source; the app's store by default. */
		optionsStore?: ProjectOptionsSource;
	}
	let { scope, url, viewer, data, onfollow, optionsStore = appOptionsStore }: Props = $props();

	const tr: Translate = (key, values) => $t(key, values ? { values } : undefined);
	const workspaceName = $derived(
		data.workspaces.items.find((w) => w.id === scope)?.name ??
			$optionsStore.items.find((p) => p.workspace_id === scope)?.workspace_name ??
			$t('shell.column.scope.workspace')
	);
	const list = $derived(
		flowList({
			scope,
			workspaceName,
			flow: viewer.flow,
			workspaceAdmin: viewer.workspaceAdmin,
			tr
		})
	);
	const selectedId = $derived(selectedRowId(list.rows, url));
</script>

{#if list.rows.length > 0}
	<NavList rows={list.rows} {selectedId} {onfollow} />
{/if}
{#if list.loading}
	<SkeletonRows thing={$t('shell.column.things.flow')} count={1} />
{/if}
{#if list.notice}
	<p class="note" data-column-note={list.notice.kind}>
		{list.notice.kind === 'off'
			? $t('shell.column.flow.off')
			: $t('shell.column.flow.unavailablePersonal')}
	</p>
	{#if list.notice.kind === 'off' && list.notice.adminHref}
		<!-- eslint-disable svelte/no-navigation-without-resolve -- built by the shell -->
		<a class="note-link" href={list.notice.adminHref} onclick={() => onfollow?.()}>
			{$t('shell.column.flow.settingsLink')}
		</a>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{/if}
{/if}

<style>
	.note {
		margin: 0;
		padding: var(--rf-space-4);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
	.note-link {
		display: block;
		padding: 0 var(--rf-space-4);
		color: var(--rf-primary);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		text-decoration: none;
	}
	.note-link:hover {
		text-decoration: underline;
	}
	.note-link:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 1px;
	}
</style>
