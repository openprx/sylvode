<script lang="ts">
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { PluginSlot } from '#lib/api/plugin-packages.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';

	let {
		workspaceId,
		projectId,
		slot,
		formKey
	}: { workspaceId: string; projectId: string; slot: PluginSlot; formKey: string } = $props();
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
</script>

{#if canManage?.()}
	<a
		href={`/workspace/${workspaceId}/projects/${projectId}/settings/plugins/creator?slot=${slot}&form=${encodeURIComponent(formKey)}`}
		data-plugin-creator-slot={slot}
		aria-label={$t('plugins.creator.launchFrom', { values: { slot: $t(`plugins.slot.${slot}`) } })}
		class="inline-flex items-center rounded-md px-2 py-1 text-sm text-blue-700 underline focus-visible:outline-blue-600 dark:text-blue-300"
		>{$t('plugins.creator.launch')}</a
	>
{/if}
