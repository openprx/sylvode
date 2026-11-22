<script lang="ts">
	import { resolve } from '$app/paths';
	import { getContext } from 'svelte';
	import { t } from 'svelte-i18n';
	import type { PluginSlotV3 } from '#lib/api/plugin-packages.js';
	import { PLUGIN_ADMIN_CONTEXT } from '#lib/plugins/context.js';
	import { slotLabelKey } from '#lib/plugins/layout-v3.js';

	let {
		workspaceId,
		projectId,
		slot,
		formKey
	}: {
		workspaceId: string;
		projectId: string;
		slot: PluginSlotV3;
		formKey: string | null;
	} = $props();
	const canManage = getContext<(() => boolean) | undefined>(PLUGIN_ADMIN_CONTEXT);
</script>

{#if canManage?.()}
	<a
		href={resolve(
			`/(app)/workspace/[workspaceId]/projects/[projectId]/settings/plugins/creator?slot=${slot}${formKey ? `&form=${encodeURIComponent(formKey)}` : ''}`,
			{ workspaceId, projectId }
		)}
		data-plugin-creator-slot={slot}
		aria-label={$t('plugins.creator.launchFrom', { values: { slot: $t(slotLabelKey(slot)) } })}
		class="inline-flex items-center rounded-control px-2 py-1 text-sm text-fg underline"
		>{$t('plugins.creator.launch')}</a
	>
{/if}
