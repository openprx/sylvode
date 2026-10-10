<script lang="ts">
	// Typed editor for one field (`versions/v0.6-collections.md`: "字段类型化编辑"). It edits an
	// editor-side `DraftValue` only; encoding to the wire type and committing are the
	// controller's job. Enter commits, Escape cancels (both reported to the parent).
	import { t } from 'svelte-i18n';
	import type { CollectionField, DraftValue } from '$lib/flow/collection-values';

	interface Props {
		field: CollectionField;
		value: DraftValue;
		id: string;
		/** Focus the control when it mounts (inline table editing). */
		autofocus?: boolean;
		onchange: (value: DraftValue) => void;
		oncommit?: (value: DraftValue) => void;
		oncancel?: () => void;
	}

	let { field, value, id, autofocus = false, onchange, oncommit, oncancel }: Props = $props();

	let host: HTMLElement | undefined = $state();

	$effect(() => {
		if (autofocus && host) {
			const control = host.matches('input,select,textarea')
				? host
				: host.querySelector<HTMLElement>('input,select,button');
			control?.focus();
		}
	});

	function onKeydown(event: KeyboardEvent): void {
		if (event.key === 'Escape') {
			event.preventDefault();
			event.stopPropagation();
			oncancel?.();
		} else if (event.key === 'Enter' && !event.shiftKey) {
			event.preventDefault();
			event.stopPropagation();
			oncommit?.(value);
		}
	}

	const inputClass =
		'min-h-11 w-full rounded border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100';

	const selected = $derived(Array.isArray(value) ? (value as readonly string[]) : []);

	function toggleOption(option: string): void {
		const next = selected.includes(option)
			? selected.filter((item) => item !== option)
			: [...selected, option];
		onchange(next);
	}
</script>

{#if field.type === 'text'}
	<input
		bind:this={host}
		{id}
		type="text"
		class={inputClass}
		value={typeof value === 'string' ? value : ''}
		aria-label={field.label}
		oninput={(event) => onchange(event.currentTarget.value)}
		onkeydown={onKeydown}
	/>
{:else if field.type === 'number'}
	<input
		bind:this={host}
		{id}
		type="text"
		inputmode="decimal"
		class={inputClass}
		value={typeof value === 'string' ? value : ''}
		aria-label={field.label}
		oninput={(event) => onchange(event.currentTarget.value)}
		onkeydown={onKeydown}
	/>
{:else if field.type === 'date'}
	<input
		bind:this={host}
		{id}
		type="date"
		class={inputClass}
		value={typeof value === 'string' ? value : ''}
		aria-label={field.label}
		oninput={(event) => onchange(event.currentTarget.value)}
		onkeydown={onKeydown}
	/>
{:else if field.type === 'boolean'}
	<label class="inline-flex min-h-11 items-center gap-2 text-sm" bind:this={host}>
		<input
			{id}
			type="checkbox"
			class="h-5 w-5"
			checked={value === true}
			aria-label={field.label}
			onchange={(event) => {
				onchange(event.currentTarget.checked);
				oncommit?.(event.currentTarget.checked);
			}}
			onkeydown={(event) => {
				if (event.key === 'Escape') onKeydown(event);
			}}
		/>
		<span>{value === true ? $t('flow.collection.value.yes') : $t('flow.collection.value.no')}</span>
	</label>
{:else if field.type === 'select'}
	<select
		bind:this={host}
		{id}
		class={inputClass}
		value={typeof value === 'string' ? value : ''}
		aria-label={field.label}
		onchange={(event) => {
			onchange(event.currentTarget.value);
			oncommit?.(event.currentTarget.value);
		}}
		onkeydown={(event) => {
			if (event.key === 'Escape') onKeydown(event);
		}}
	>
		<option value="">{$t('flow.collection.value.none')}</option>
		{#each field.options as option (option)}
			<option value={option}>{option}</option>
		{/each}
	</select>
{:else if field.type === 'multi_select'}
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<div
		bind:this={host}
		{id}
		role="group"
		aria-label={field.label}
		class="flex flex-wrap gap-1"
		tabindex="-1"
		onkeydown={onKeydown}
	>
		{#if field.options.length === 0}
			<span class="text-xs text-slate-500">{$t('flow.collection.value.noOptions')}</span>
		{/if}
		{#each field.options as option (option)}
			<button
				type="button"
				class="min-h-11 rounded border px-2 text-sm focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 {selected.includes(
					option
				)
					? 'border-blue-500 bg-blue-50 text-blue-800 dark:bg-blue-950 dark:text-blue-100'
					: 'border-slate-300 dark:border-slate-600'}"
				aria-pressed={selected.includes(option)}
				onclick={() => toggleOption(option)}>{option}</button
			>
		{/each}
	</div>
{:else}
	<span class="text-xs text-slate-500" bind:this={host}>{$t('flow.collection.value.readOnly')}</span
	>
{/if}
