<script lang="ts">
	import PageHeader from '#lib/ui/page-header/PageHeader.svelte';
	import type { StandaloneSurfaceHost } from '#lib/surface/standalone-host.svelte.js';

	interface Props {
		host: StandaloneSurfaceHost;
	}
	let { host }: Props = $props();

	let header = $state<ReturnType<typeof PageHeader> | null>(null);
	const title = $derived(host.title);
	// The header has no separate identifier slot: the identifier leads the title.
	const text = $derived(
		title ? (title.identifier ? `${title.identifier} ${title.title}` : title.title) : ''
	);

	/** Moves focus to the title (after a client navigation to a surface). */
	export function focusTitle(): void {
		header?.focusTitle();
	}
</script>

<PageHeader
	bind:this={header}
	title={text}
	variant={host.header === 'compact' ? 'compact' : 'standard'}
	context={title?.context}
	loading={host.loading}
	primary={host.actions.primary}
	secondary={host.actions.secondary}
/>
