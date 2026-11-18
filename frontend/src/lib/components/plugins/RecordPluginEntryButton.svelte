<script lang="ts">
	import type { RecordPluginEntry } from '#lib/plugins/placements.js';

	let {
		entry,
		expanded,
		panelId,
		onToggle,
		showVersion = false
	}: {
		entry: RecordPluginEntry;
		expanded: boolean;
		panelId: string;
		onToggle: (entry: RecordPluginEntry, trigger: HTMLElement) => void;
		showVersion?: boolean;
	} = $props();
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
	{#if showVersion}<span class="text-xs text-slate-600 dark:text-slate-300"
			>v{entry.panel.version}</span
		>{/if}
</button>
