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
		local: 'bg-slate-400',
		saving: 'bg-amber-500 animate-pulse motion-reduce:animate-none',
		saved: 'bg-emerald-500',
		offline: 'bg-slate-400',
		reconnecting: 'bg-amber-500 animate-pulse motion-reduce:animate-none',
		resyncing: 'bg-amber-500 animate-pulse motion-reduce:animate-none',
		auth_required: 'bg-red-500',
		read_only: 'bg-slate-400',
		error: 'bg-red-500'
	};

	const dotClass = $derived(DOT_CLASS[state]);
	const label = $derived($t(`flow.sync.${state}`));
	const pendingLabel = $derived(
		pending > 0 ? $t('flow.sync.pending', { values: { count: pending } }) : ''
	);
</script>

<div
	class="flex items-center gap-2 rounded-full border border-slate-200 bg-white px-3 py-1 text-xs font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300"
	role="status"
	aria-live="polite"
	data-testid="flow-sync-indicator"
	data-state={state}
	data-pending={pending}
>
	<span class={`h-2 w-2 rounded-full ${dotClass}`} aria-hidden="true"></span>
	<span>{label}</span>
	{#if pendingLabel}
		<span data-testid="flow-sync-pending">{pendingLabel}</span>
	{/if}
</div>
