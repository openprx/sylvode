<script lang="ts">
	// The recovery draft, persistently visible next to the canvas (`ui-surface-v1.md`: "sync/auth/
	// recovery/limit 状态必须在 canvas 邻近持续可见"). Lists every intent that could not be replayed
	// and why; the download and copy actions are the caller's (`ObjectRepository` builds both).
	import { t } from 'svelte-i18n';
	import type { RecoveryDraftEntry } from '$lib/flow/outbox';
	import { flowErrorDetailKey } from '$lib/flow/errors';

	interface Props {
		entries: ReadonlyArray<RecoveryDraftEntry>;
		/** The session was given up for good (three consecutive `server_rejected`). */
		permanent?: boolean;
		ondownload: () => void;
		oncopy: () => void;
		ondiscard: () => void;
	}

	let { entries, permanent = false, ondownload, oncopy, ondiscard }: Props = $props();

	function reasonValues(entry: RecoveryDraftEntry): Record<string, string | number> {
		const details = entry.reason.details;
		if (typeof details !== 'object' || details === null) return {};
		const out: Record<string, string | number> = {};
		for (const [key, value] of Object.entries(details as Record<string, unknown>)) {
			if (typeof value === 'string' || typeof value === 'number') out[key] = value;
		}
		return out;
	}

	const SOURCE_KEYS: Record<RecoveryDraftEntry['reason']['source'], string> = {
		server: 'flow.recovery.source.server',
		precheck: 'flow.recovery.source.precheck',
		dependency: 'flow.recovery.source.dependency'
	};

	function intentKey(entry: RecoveryDraftEntry): string {
		return entry.intent.kind === 'navigator_reorder'
			? 'flow.recovery.intent.navigator_reorder'
			: 'flow.recovery.intent.content_edit';
	}
</script>

<section
	class="border-b border-red-200 bg-red-50 px-6 py-3 text-sm text-red-900 dark:border-red-900 dark:bg-red-950 dark:text-red-100"
	aria-label={$t('flow.recovery.title')}
	data-testid="flow-recovery-draft"
	data-count={entries.length}
>
	<div class="flex flex-wrap items-center gap-3">
		<p class="flex-1 font-medium" aria-live="polite">
			{permanent
				? $t('flow.recovery.permanent', { values: { count: entries.length } })
				: $t('flow.recovery.summary', { values: { count: entries.length } })}
		</p>
		<button
			type="button"
			class="min-h-11 rounded px-3 font-medium underline underline-offset-2 hover:no-underline focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
			data-testid="flow-recovery-download"
			onclick={ondownload}
		>
			{$t('flow.recovery.downloadJson')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded px-3 font-medium underline underline-offset-2 hover:no-underline focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
			data-testid="flow-recovery-copy"
			onclick={oncopy}
		>
			{$t('flow.recovery.copyText')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded px-3 font-medium underline underline-offset-2 hover:no-underline focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
			data-testid="flow-recovery-discard"
			onclick={ondiscard}
		>
			{$t('flow.recovery.discard')}
		</button>
	</div>
	<ol class="mt-2 list-decimal space-y-0.5 pl-6 text-xs">
		{#each entries as entry (entry.updateId)}
			<li
				data-testid="flow-recovery-entry"
				data-reason={entry.reason.code}
				data-source={entry.reason.source}
			>
				{$t(intentKey(entry))} &middot;
				{$t(flowErrorDetailKey(entry.reason), { values: reasonValues(entry) })}
				({$t(SOURCE_KEYS[entry.reason.source])})
			</li>
		{/each}
	</ol>
</section>
