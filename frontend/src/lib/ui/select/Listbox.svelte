<script lang="ts" generics="V extends string = string">
	import type { Snippet } from 'svelte';
	import '../foundations/visually-hidden.css';
	import { t } from 'svelte-i18n';
	import Check from '@lucide/svelte/icons/check';
	import Search from '@lucide/svelte/icons/search';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import { optionId, type SelectOption } from './listbox';

	type Status = 'ready' | 'loading' | 'failed';

	interface Props {
		id: string;
		label: string;
		options: SelectOption<V>[];
		activeIndex: number;
		multiple?: boolean;
		isSelected: (option: SelectOption<V>) => boolean;
		searchable?: boolean;
		query?: string;
		status?: Status;
		compact?: boolean;
		/** True while the active option was reached with the keyboard (it then shows the ring). */
		keyboard?: boolean;
		/** Without a search field the trigger owns the keys; with one the field forwards them. */
		onsearchkey?: (event: KeyboardEvent) => void;
		onpick: (index: number) => void;
		onhover?: (index: number) => void;
		onretry?: () => void;
		/** The element that holds focus while the list is open (search input), for the opener. */
		searchEl?: HTMLInputElement | null;
		footer?: Snippet;
	}

	let {
		id,
		label,
		options,
		activeIndex,
		multiple = false,
		isSelected,
		searchable = false,
		query = $bindable(''),
		status = 'ready',
		compact = false,
		keyboard = false,
		onsearchkey,
		onpick,
		onhover,
		onretry,
		searchEl = $bindable(null),
		footer
	}: Props = $props();

	let listEl = $state<HTMLElement | null>(null);

	$effect(() => {
		if (!listEl || activeIndex < 0) return;
		const el = listEl.querySelector<HTMLElement>(`#${CSS.escape(optionId(id, activeIndex))}`);
		el?.scrollIntoView?.({ block: 'nearest' });
	});
</script>

<div class="wrap" class:compact>
	{#if searchable}
		<div class="search">
			<Search size={16} aria-hidden="true" />
			<input
				bind:this={searchEl}
				bind:value={query}
				type="text"
				role="combobox"
				aria-expanded="true"
				aria-controls={id}
				aria-label={$t('ui.select.search')}
				aria-activedescendant={activeIndex >= 0 && options.length > 0
					? optionId(id, activeIndex)
					: undefined}
				placeholder={$t('ui.select.search')}
				autocomplete="off"
				spellcheck="false"
				onkeydown={onsearchkey}
			/>
		</div>
	{/if}
	{#if status === 'ready' && options.length > 0}
		<div
			bind:this={listEl}
			{id}
			role="listbox"
			aria-label={label}
			aria-multiselectable={multiple || undefined}
			class="list"
			class:with-footer={!!footer}
		>
			{#each options as option, index (option.value)}
				{@const selected = isSelected(option)}
				<!-- svelte-ignore a11y_click_events_have_key_events -->
				<div
					id={optionId(id, index)}
					role="option"
					tabindex="-1"
					aria-selected={selected}
					aria-disabled={option.disabled || undefined}
					class="option"
					class:active={index === activeIndex}
					class:keyboard
					data-value={option.value}
					onpointerdown={(e) => e.preventDefault()}
					onpointermove={() => onhover?.(index)}
					onclick={() => onpick(index)}
				>
					{#if multiple}
						<span class="box" class:checked={selected} aria-hidden="true">
							{#if selected}<Check size={13} />{/if}
						</span>
					{:else}
						<span class="slot" aria-hidden="true">
							{#if selected}<Check size={16} />{/if}
						</span>
					{/if}
					{#if option.color}<span class="dot" style:background="var(--rf-{option.color})"
						></span>{/if}
					<span class="text" title={option.label}>{option.label}</span>
					{#if option.meta}<span class="meta">{option.meta}</span>{/if}
					{#if option.disabled && option.disabledReason}
						<span class="ui-sr-only"
							>{$t('ui.select.unavailable', { values: { reason: option.disabledReason } })}</span
						>
					{/if}
				</div>
			{/each}
		</div>
	{:else if status === 'loading'}
		<div class="status" role="status">
			<span class="ui-sr-only">{$t('ui.select.loading')}</span>
			{#each [0, 1, 2] as row (row)}<span class="skeleton" aria-hidden="true"></span>{/each}
		</div>
	{:else if status === 'failed'}
		<div class="failed" role="status">
			<CircleAlert size={14} aria-hidden="true" />
			<span class="text">{$t('ui.select.failed')}</span>
			<button
				type="button"
				class="retry"
				onpointerdown={(e) => e.preventDefault()}
				onclick={() => onretry?.()}
			>
				{$t('ui.select.retry')}
			</button>
		</div>
	{:else}
		<div class="empty" role="status">{$t('ui.select.noMatch', { values: { q: query } })}</div>
	{/if}
	{#if footer}{@render footer()}{/if}
</div>

<style>
	.wrap {
		display: flex;
		flex-direction: column;
		max-width: 360px;
		font-family: var(--rf-font-sans-stack);
	}
	.search {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		box-sizing: border-box;
		height: 34px;
		margin: 0 0 var(--rf-space-2);
		padding: 0 var(--rf-space-4);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-secondary);
	}
	.search:focus-within {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.search input {
		flex: 1;
		min-width: 0;
		height: 100%;
		padding: 0;
		border: 0;
		outline: 0;
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-secondary);
	}
	.search input::placeholder {
		color: var(--rf-secondary);
	}
	.list {
		max-height: calc(var(--rf-menu-item-height) * 8.5);
		overflow-y: auto;
	}
	.compact .list {
		max-height: calc(32px * 8.5);
	}
	.option {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		min-height: var(--rf-menu-item-height);
		padding: 0 var(--rf-space-5) 0 var(--rf-space-4);
		border-radius: var(--rf-radius-sm);
		color: var(--rf-text);
		font-size: var(--rf-size-button);
		line-height: var(--rf-leading-button);
		cursor: pointer;
		user-select: none;
	}
	.compact .option {
		min-height: 32px;
		font-size: var(--rf-size-secondary);
	}
	.option:hover {
		background: var(--rf-surface);
	}
	.option:active {
		background: var(--rf-pressed);
	}
	.option.active.keyboard {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.option[aria-disabled='true'] {
		color: var(--rf-secondary);
		cursor: not-allowed;
	}
	.option[aria-disabled='true']:hover {
		background: transparent;
	}
	.slot {
		display: inline-flex;
		flex: none;
		width: 16px;
		height: 16px;
	}
	.box {
		display: inline-grid;
		flex: none;
		place-items: center;
		box-sizing: border-box;
		width: 18px;
		height: 18px;
		border: var(--rf-border-width-control) solid var(--rf-control);
		border-radius: var(--rf-radius-xs);
		background: var(--rf-input);
	}
	.box.checked {
		border-color: var(--rf-primary);
		background: var(--rf-primary);
		color: var(--rf-on-primary);
	}
	.option[aria-disabled='true'] .box {
		border-color: var(--rf-line-strong);
		background: var(--rf-surface);
	}
	.dot {
		flex: none;
		width: 8px;
		height: 8px;
		border-radius: var(--rf-radius-full);
	}
	.text {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.meta {
		flex: none;
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.empty {
		padding: var(--rf-space-5);
		font-size: var(--rf-size-secondary);
		color: var(--rf-secondary);
	}
	.status {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		padding: var(--rf-space-4);
	}
	.skeleton {
		height: 14px;
		border-radius: var(--rf-radius-xs);
		background: var(--rf-surface);
	}
	.failed {
		display: flex;
		align-items: center;
		gap: var(--rf-space-4);
		margin: var(--rf-space-2);
		padding: var(--rf-space-4) var(--rf-space-5);
		border-radius: var(--rf-radius-sm);
		background: var(--rf-danger-bg);
		color: var(--rf-text);
		font-size: var(--rf-size-secondary);
	}
	.failed :global(svg) {
		flex: none;
		color: var(--rf-danger);
	}
	.retry {
		flex: none;
		padding: 0 var(--rf-space-3);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		font-weight: var(--rf-weight-medium);
		text-decoration: underline;
		cursor: pointer;
	}
	.retry:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	@media (pointer: coarse) {
		.option {
			min-height: var(--rf-touch-target);
		}
	}
</style>
