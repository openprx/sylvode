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
	} from '#lib/flow/history-diff.js';

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
		title: 'text-fg',
		added: 'text-success',
		removed: 'text-danger',
		changed: 'text-warning',
		moved: 'text-fg-muted'
	};
</script>

<div class="mt-2 space-y-1 text-xs" data-testid="flow-history-diff" aria-live="polite">
	{#if result === null}
		<p class="text-fg-muted">{$t('flow.history.diff.loading')}</p>
	{:else if result.status === 'failed'}
		<p class="text-danger" role="alert">{$t(result.messageKey)}</p>
	{:else if result.lines.length === 0}
		<p class="text-fg-muted">{$t('flow.history.diff.empty')}</p>
	{:else}
		<ul class="space-y-1">
			{#each result.lines as line, index (index)}
				<li class={opClass[line.op]} data-testid="flow-history-diff-line" data-op={line.op}>
					<span class="font-semibold">{$t(DIFF_OPERATION_KEYS[line.op])}</span>
					{#if line.kind}
						<span class="text-fg-muted">· {$t(diffKindKey(line.kind))}</span>
					{/if}
					{#if line.op === 'added'}
						<span class="block break-words">{shown(line.after)}</span>
					{:else if line.op === 'removed'}
						<span class="block break-words line-through">{shown(line.before)}</span>
					{:else if line.properties.some((change) => change.key === 'spans' && change.before === null && change.after === null) && line.before === line.after}
						<span class="block break-words"
							>{shown(line.after)} · {$t('flow.history.diff.formatting')}</span
						>
					{:else if line.op === 'title' || line.op === 'changed'}
						<span class="block break-words">
							{$t('flow.history.diff.arrow', {
								values: { before: shown(line.before), after: shown(line.after) }
							})}
						</span>
					{/if}
					{#each line.properties as property (property.key)}
						{#if property.key !== 'spans' || property.before !== null || property.after !== null}
							<span class="block break-words text-fg-muted">
								{$t('flow.history.diff.property', {
									values: {
										key: property.key,
										before: shown(property.before),
										after: shown(property.after)
									}
								})}
							</span>
						{/if}
					{/each}
				</li>
			{/each}
		</ul>
	{/if}
</div>
