<script lang="ts">
	// Read-only rendering of one stored value. Placeholders (restricted, empty, invalid) come from
	// `displayValue`, which only reads `values_by_field_id` (`ADR-0018` NR-7: never guessed).
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import {
		CELL_PLACEHOLDER_KEYS,
		displayValue,
		type CollectionField
	} from '#lib/flow/collection-values.js';

	interface Props {
		field: CollectionField;
		values: Readonly<Record<string, unknown>>;
		workspaceId: string;
	}

	let { field, values, workspaceId }: Props = $props();

	const display = $derived(displayValue(field, values));
</script>

{#if display.kind === 'restricted' || display.kind === 'empty' || display.kind === 'invalid'}
	<span
		class="text-slate-400 italic dark:text-slate-500"
		data-cell-placeholder={display.kind}
		title={$t(CELL_PLACEHOLDER_KEYS[display.kind])}
	>
		{display.kind === 'empty' ? '—' : $t(CELL_PLACEHOLDER_KEYS[display.kind])}
		<span class="sr-only">{display.kind === 'empty' ? $t(CELL_PLACEHOLDER_KEYS.empty) : ''}</span>
	</span>
{:else if display.kind === 'text'}
	<span class="truncate">{display.text}</span>
{:else if display.kind === 'boolean'}
	<span>{display.value ? $t('flow.collection.value.yes') : $t('flow.collection.value.no')}</span>
{:else if display.kind === 'options'}
	<span class="flex flex-wrap gap-1">
		{#each display.values as option (option)}
			<span
				class="rounded bg-slate-100 px-1.5 py-0.5 text-xs text-slate-700 dark:bg-slate-800 dark:text-slate-200"
				>{option}</span
			>
		{/each}
	</span>
{:else if display.kind === 'relation'}
	<span class="flex flex-wrap gap-1">
		{#each display.ids as id (id)}
			<a
				href={resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
					workspaceId,
					objectId: id
				})}
				class="text-xs text-blue-600 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-400"
				>{$t('flow.collection.value.relationLink', { values: { id: id.slice(0, 8) } })}</a
			>
		{/each}
	</span>
{/if}
