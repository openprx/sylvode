<script lang="ts">
	import { isContributionEntry, type RecordEntry } from '#lib/plugins/placements.js';

	let {
		entry,
		expanded,
		panelId,
		onToggle,
		showVersion = false
	}: {
		entry: RecordEntry;
		expanded: boolean;
		panelId: string;
		onToggle: (entry: RecordEntry, trigger: HTMLElement) => void;
		showVersion?: boolean;
	} = $props();
	const version = $derived(isContributionEntry(entry) ? entry.item.version : entry.panel.version);
</script>

<button
	type="button"
	class="rounded-md border border-slate-300 px-3 py-1.5 text-sm hover:bg-slate-50 focus-visible:outline-blue-600 dark:border-slate-600 dark:hover:bg-slate-800"
	data-plugin-contribution={entry.id}
	aria-expanded={expanded}
	aria-controls={expanded ? panelId : undefined}
	onclick={(event) => onToggle(entry, event.currentTarget)}
>
	{entry.label}
	{#if showVersion}<span class="text-xs text-slate-600 dark:text-slate-300">v{version}</span>{/if}
</button>
