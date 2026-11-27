<svelte:options runes />

<script lang="ts">
	import { projectDiscovery, type DiscoveryState } from '#lib/plugins/contributions.js';
	import { projectOptionsStore } from '#lib/stores/project-options.js';
	import { CUSTOM_FORM_TYPE } from '#lib/project/nav.js';
	import ProjectPluginNav from './ProjectPluginNav.svelte';

	/**
	 * The apps of a form-centred project on its forms page. Such a project has no overview, which
	 * is where every other project type shows its apps, so without this strip a member could not
	 * reach an installed app from the project at all.
	 */
	let { workspaceId, projectId }: { workspaceId: string; projectId: string } = $props();

	let discovery = $state.raw<DiscoveryState>({ status: 'loading', items: [] });

	$effect(() => {
		const current = projectDiscovery(projectId);
		const stop = current.subscribe((value) => (discovery = value));
		const release = current.retain();
		return () => {
			stop();
			release();
		};
	});

	const typeKey = $derived($projectOptionsStore.items.find((p) => p.id === projectId)?.type_key);
</script>

{#if typeKey === CUSTOM_FORM_TYPE && discovery.status === 'ready' && discovery.items.length > 0}
	<ProjectPluginNav {workspaceId} {projectId} />
{/if}
