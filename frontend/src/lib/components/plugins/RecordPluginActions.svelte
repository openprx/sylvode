<script lang="ts">
	import { t } from 'svelte-i18n';
	import RecordPluginEntryButton from './RecordPluginEntryButton.svelte';
	import {
		entryPluginKey,
		sameEntry,
		type RecordPluginActionSurface
	} from '#lib/plugins/placements.js';

	let { recordId, surface }: { recordId: string; surface: RecordPluginActionSurface } = $props();
	const actions = $derived($surface?.recordId === recordId ? $surface : null);
</script>

{#if actions?.entries.length || actions?.more.length}
	<div class="flex flex-wrap items-center justify-end gap-2" data-plugin-slot="form.record.actions">
		{#each actions.entries as entry (`${entryPluginKey(entry)}:${entry.id}`)}
			<RecordPluginEntryButton
				{entry}
				panelId={actions.panelId}
				expanded={sameEntry(actions.active, entry)}
				onToggle={actions.toggle}
			/>
		{/each}
		{#if actions.more.length}
			<details class="relative">
				<summary
					class="cursor-pointer rounded-md border border-slate-300 px-3 py-1.5 text-sm dark:border-slate-600"
					>{$t('plugins.host.more')}</summary
				>
				<ul
					class="absolute right-0 z-20 mt-1 min-w-44 space-y-1 rounded-md border border-slate-300 bg-white p-2 shadow dark:border-slate-600 dark:bg-slate-900"
				>
					{#each actions.more as entry (`${entryPluginKey(entry)}:${entry.id}`)}
						<li>
							<RecordPluginEntryButton
								{entry}
								panelId={actions.panelId}
								expanded={sameEntry(actions.active, entry)}
								onToggle={(next, trigger) => {
									const details = trigger.closest('details');
									if (details) details.open = false;
									actions.toggle(next, trigger);
								}}
							/>
						</li>
					{/each}
				</ul>
			</details>
		{/if}
	</div>
{/if}
