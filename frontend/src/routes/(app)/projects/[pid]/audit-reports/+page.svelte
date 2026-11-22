<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';

	let { data }: { data: { notFound?: boolean } } = $props();
</script>

{#if data.notFound}
	<StateBlock
		kind="notFound"
		object={$t('workbench.project').toLowerCase()}
		place={$t('workbench.projects').toLowerCase()}
		action={{ run: () => goto(resolve('/(app)/workspace')) }}
	/>
{:else}
	<StateBlock kind="loading" thing={$t('workbench.project').toLowerCase()} />
{/if}
