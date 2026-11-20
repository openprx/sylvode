<script lang="ts">
	import { t } from 'svelte-i18n';
	import { page } from '$app/state';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import { recordSurface } from '#lib/surface/registry.js';

	const params = $derived(recordSurface.params(new URL(page.url.href)));
	const Surface = recordSurface.component;
</script>

{#if params}
	{#key `${params.workspaceId}/${params.projectId}/${params.recordId}`}
		<Surface {...params} />
	{/key}
{:else}
	<StateBlock kind="notFound" message={$t('surface.embed.invalidLink')} />
{/if}
