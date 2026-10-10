<script lang="ts">
	// The semantic diff of one history entry. Loading and shaping are in `lib/flow/history-diff.ts`;
	// lines carry node kind, text and property changes only, never node ids, peer ids or bytes.
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		DIFF_OPERATION_KEYS,
		diffKindKey,
		type HistoryDiffLoader,
		type HistoryDiffResult
	} from '$lib/flow/history-diff';

	interface Props {
		seq: number;
		loader: HistoryDiffLoader;
	}

	let { seq, loader }: Props = $props();

	let result = $state<HistoryDiffResult | null>(null);

	onMount(() => {
		void loader.load(seq).then((value) => (result = value));
	});

	function shown(value: string | null): string {
		return value === null || value === '' ? '∅' : value;
	}

	const opClass: Record<string, string> = {
		title: 'text-slate-700 dark:text-slate-200',
		added: 'text-emerald-700 dark:text-emerald-400',
		removed: 'text-red-700 dark:text-red-400',
		changed: 'text-amber-700 dark:text-amber-400',
		moved: 'text-slate-600 dark:text-slate-300'
	};
</script>

<div class="mt-2 space-y-1 text-xs" data-testid="flow-history-diff" aria-live="polite">
	{#if result === null}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.history.diff.loading')}</p>
	{:else if result.status === 'failed'}
		<p class="text-red-600 dark:text-red-400" role="alert">{$t(result.messageKey)}</p>
	{:else if result.lines.length === 0}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.history.diff.empty')}</p>
	{:else}
		<ul class="space-y-1">
			{#each result.lines as line, index (index)}
				<li class={opClass[line.op]} data-testid="flow-history-diff-line" data-op={line.op}>
					<span class="font-semibold">{$t(DIFF_OPERATION_KEYS[line.op])}</span>
					{#if line.kind}
						<span class="text-slate-400">· {$t(diffKindKey(line.kind))}</span>
					{/if}
					{#if line.op === 'added'}
						<span class="block break-words">{shown(line.after)}</span>
					{:else if line.op === 'removed'}
						<span class="block break-words line-through">{shown(line.before)}</span>
					{:else if line.op === 'title' || line.op === 'changed'}
						<span class="block break-words">
							{$t('flow.history.diff.arrow', {
								values: { before: shown(line.before), after: shown(line.after) }
							})}
						</span>
					{/if}
					{#each line.properties as property (property.key)}
						<span class="block break-words text-slate-500 dark:text-slate-400">
							{$t('flow.history.diff.property', {
								values: {
									key: property.key,
									before: shown(property.before),
									after: shown(property.after)
								}
							})}
						</span>
					{/each}
				</li>
			{/each}
		</ul>
	{/if}
</div>
