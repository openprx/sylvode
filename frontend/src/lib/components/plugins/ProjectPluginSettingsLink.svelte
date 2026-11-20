<script lang="ts">
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';

	let { workspaceId, projectId }: { workspaceId: string; projectId: string } = $props();
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
</script>

{#if canManage?.()}
	<a
		class="rounded-md px-4 py-2 text-sm font-medium text-blue-700 hover:bg-blue-50 dark:text-blue-300 dark:hover:bg-slate-800"
		href={`/workspace/${workspaceId}/projects/${projectId}/settings/plugins`}
		>{$t('plugins.projectSettings')}</a
	>
{/if}
