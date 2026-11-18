<script lang="ts">
	import { pageInlineRuns, type PageInlineSpan } from '#lib/flow/inline-runs.js';
	let { text, spans = [] }: { text: string; spans?: readonly PageInlineSpan[] } = $props();
	const runs = $derived(pageInlineRuns(text, spans));
	const tags = { bold: 'strong', italic: 'em', strike: 's', code: 'code' } as const;
</script>

{#snippet styled(
	value: string,
	marks: { mark: PageInlineSpan['mark']; href?: string }[],
	index: number
)}
	{#if index >= marks.length}{value}
	{:else if marks[index].mark === 'link'}
		<!-- Stored content URLs are validated by pageInlineRuns, not application route templates. -->
		<!-- eslint-disable svelte/no-navigation-without-resolve -->
		<a href={marks[index].href} rel="noopener noreferrer" target="_blank"
			>{@render styled(value, marks, index + 1)}</a
		>
		<!-- eslint-enable svelte/no-navigation-without-resolve -->
	{:else}<svelte:element this={tags[marks[index].mark as keyof typeof tags]}
			>{@render styled(value, marks, index + 1)}</svelte:element
		>{/if}
{/snippet}
{#each runs as run, index (index)}{@render styled(run.text, run.marks, 0)}{/each}
