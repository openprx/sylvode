<script lang="ts">
	import type { RecordEntry } from '#lib/plugins/record-entries.js';

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
	const version = $derived(entry.item.version);
</script>

<button
	type="button"
	class="rounded-control border border-input px-3 py-1.5 text-sm hover:bg-surface"
	data-plugin-contribution={entry.id}
	aria-expanded={expanded}
	aria-controls={expanded ? panelId : undefined}
	onclick={(event) => onToggle(entry, event.currentTarget)}
>
	{entry.label}
	{#if showVersion}<span class="text-xs text-fg-muted">v{version}</span>{/if}
</button>
