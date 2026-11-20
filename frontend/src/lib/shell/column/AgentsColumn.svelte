<script lang="ts">
	import type { ScopeKey } from '../destinations';
	import AgentsLists from './AgentsLists.svelte';
	import type { ColumnData } from './column-data.svelte';
	import type { NavRow } from './rows';
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
	let { scope, url, viewer, data, onfollow, optionsStore }: Props = $props();
</script>

<AgentsLists
	{scope}
	{url}
	workspaceAdmin={viewer.workspaceAdmin}
	{data}
	{onfollow}
	{optionsStore}
/>
