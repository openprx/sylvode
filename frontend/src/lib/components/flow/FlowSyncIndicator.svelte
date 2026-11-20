<script lang="ts">
	import { t } from 'svelte-i18n';
	import type { SyncState } from '#lib/flow/types.js';

	interface Props {
		state: SyncState;
		/** Local updates waiting for the server (offline edits, or an in-flight save). */
		pending?: number;
	}

	let { state, pending = 0 }: Props = $props();

	// `motion-reduce:animate-none` (`contracts/ui-surface-v1.md` a11y baseline, task brief item 10):
	// `prefers-reduced-motion: reduce` must stop the pulsing dot -- the sync state is still
	// conveyed by color and the text label next to it, never by motion alone.
	const DOT_CLASS: Record<SyncState, string> = {
		local: 'bg-control',
		saving: 'bg-warning animate-pulse motion-reduce:animate-none',
		saved: 'bg-success',
		offline: 'bg-control',
		reconnecting: 'bg-warning animate-pulse motion-reduce:animate-none',
		resyncing: 'bg-warning animate-pulse motion-reduce:animate-none',
		auth_required: 'bg-danger',
		read_only: 'bg-control',
		error: 'bg-danger'
	};

	const displayedState = $derived(state === 'saved' && pending > 0 ? 'saving' : state);
	const dotClass = $derived(DOT_CLASS[displayedState]);
	const label = $derived($t(`flow.sync.${displayedState}`));
	const pendingLabel = $derived(
		pending > 0 ? $t('flow.sync.pending', { values: { count: pending } }) : ''
	);
</script>

<div
	class="flex items-center gap-2 rounded-full border border-line bg-raised px-3 py-1 text-xs font-medium text-fg-muted"
	role="status"
	aria-live="polite"
	data-testid="flow-sync-indicator"
	data-state={displayedState}
	data-pending={pending}
>
	<span class={`h-2 w-2 rounded-full ${dotClass}`} aria-hidden="true"></span>
	<span>{label}</span>
	{#if pendingLabel}
		<span data-testid="flow-sync-pending">{pendingLabel}</span>
	{/if}
</div>
