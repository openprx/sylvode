<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { inboundApi } from '#lib/api/inbound.js';
	import SectionFrame from './SectionFrame.svelte';
	import { loadStateOf, type LoadState } from './load-state';

	interface Props {
		workspaceId: string;
		projectId: string;
	}
	// workspaceId is part of the section contract; the filled-in section builds its links with it
	// eslint-disable-next-line svelte/no-unused-props
	let { projectId }: Props = $props();

	let loadState = $state<LoadState>('loading');
	let inboundEnabled = $state(true);

	async function load(): Promise<void> {
		const result = await inboundApi.listSources(projectId);
		if (result.ok) inboundEnabled = result.value.inbound_enabled;
		loadState = loadStateOf(result);
	}

	onMount(() => void load());
</script>

<SectionFrame
	titleKey="inbound.rules.title"
	descriptionKey="inbound.rules.description"
	thingKey="inbound.rules.thing"
	state={loadState}
	{inboundEnabled}
	testId="alert-rules-section"
	onretry={() => {
		loadState = 'loading';
		void load();
	}}
>
	<!-- The filled-in section replaces this body. -->
</SectionFrame>
