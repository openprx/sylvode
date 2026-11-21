<script lang="ts">
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';

	let { workspaceId, projectId }: { workspaceId: string; projectId: string } = $props();
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
</script>

{#if canManage?.()}
	<a
		class="rounded-[var(--radius-sm)] px-4 py-2 text-sm font-medium text-fg hover:bg-surface"
		href={`/workspace/${workspaceId}/projects/${projectId}/settings/plugins`}
		>{$t('plugins.projectSettings')}</a
	>
{/if}
