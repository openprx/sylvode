<script lang="ts">
	import RecordPluginEntryButton from './RecordPluginEntryButton.svelte';
	import { sameEntry, type RecordPluginActionSurface } from '#lib/plugins/placements.js';

	let { recordId, surface }: { recordId: string; surface: RecordPluginActionSurface } = $props();
	const actions = $derived($surface?.recordId === recordId ? $surface : null);
</script>

{#if actions?.entries.length}
	<div class="flex flex-wrap items-center justify-end gap-2" data-plugin-slot="form.record.actions">
		{#each actions.entries as entry (`${entry.panel.key}:${entry.id}`)}
			<RecordPluginEntryButton
				{entry}
				panelId={actions.panelId}
				expanded={sameEntry(actions.active, entry)}
				onToggle={actions.toggle}
			/>
		{/each}
	</div>
{/if}
