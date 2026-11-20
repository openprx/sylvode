<script lang="ts" generics="V extends string = string">
	/**
	 * The shared body of Select and MultiSelect: the field, the trigger, the popover list, the keys.
	 * Both public components wrap it; `values` is always an array here (zero or one for Select).
	 */
	import { untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import X from '@lucide/svelte/icons/x';
	import Field from '../form/Field.svelte';
	import type { Marking } from '../form/field-state';
	import Popover from '../popover/Popover.svelte';
	import { uid } from '../foundations/ids';
	import { backspace, chipView, summaryName, toggleValue } from '../multi-select/multi-state';
	import Listbox from './Listbox.svelte';
	import {
		filterOptions,
		initialActive,
		initialTypeahead,
		keyAction,
		moveActive,
		optionId,
		shouldSearch,
		typeahead,
		type KeyAction,
		type SelectOption
	} from './listbox';

	interface Props {
		values: V[];
		multiple?: boolean;
		options: SelectOption<V>[] | (() => Promise<SelectOption<V>[]>);
		onvalues: (values: V[]) => void;
		label: string;
		labelHidden?: boolean;
		placeholder?: string;
		helper?: string;
		error?: string;
		required?: boolean;
		marking?: Marking;
		disabled?: boolean;
		disabledReason?: string;
		readonly?: boolean;
		searchable?: boolean | 'auto';
		density?: 'comfortable' | 'compact';
		nativeOnTouch?: boolean;
		/** Single select only: list "None" first; choosing it gives `null`. */
		allowEmpty?: boolean;
		maxChips?: number;
		id?: string;
		testId?: string;
	}

	let {
		values,
		multiple = false,
		options,
		onvalues,
		label,
		labelHidden = false,
		placeholder,
		helper,
		error,
		required = false,
		marking,
		disabled = false,
		disabledReason,
		readonly = false,
		searchable = 'auto',
		density = 'comfortable',
		nativeOnTouch = true,
		allowEmpty = false,
		maxChips = 2,
		id = uid('select'),
		testId
	}: Props = $props();

	const NONE = '__ui-none__';
	const listboxId = $derived(`${id}-listbox`);

	let open = $state(false);
	let query = $state('');
	let activeIndex = $state(-1);
	let keyboard = $state(false);
	let box = $state<HTMLElement | null>(null);
	let trigger = $state<HTMLButtonElement | null>(null);
	let searchEl = $state<HTMLInputElement | null>(null);
	let menuWidth = $state(220);
	let loaded = $state<SelectOption<V>[]>([]);
	let loadStatus = $state<'ready' | 'loading' | 'failed'>('loading');
	let marked = $state<V | null>(null);
	let announcement = $state('');
	let native = $state(false);
	let typeState = initialTypeahead;
	let loadToken = 0;

	const compact = $derived(density === 'compact');
	const source = $derived(Array.isArray(options) ? options : loaded);
	const status = $derived(Array.isArray(options) ? 'ready' : loadStatus);
	const items = $derived<SelectOption<string>[]>(
		allowEmpty && !multiple ? [{ value: NONE, label: $t('ui.select.none') }, ...source] : source
	);
	const filtered = $derived(filterOptions(items, query));
	const hasSearch = $derived(shouldSearch(searchable, source.length));
	const labelOf = (value: string) => source.find((o) => o.value === value)?.label ?? value;
	const selectedItems = $derived(
		values.map((value) => ({ value: value as string, label: labelOf(value) }))
	);
	const singleLabel = $derived(
		values.length > 0 && (status === 'ready' || source.some((o) => o.value === values[0]))
			? labelOf(values[0])
			: allowEmpty && values.length === 0
				? $t('ui.select.none')
				: ''
	);
	const view = $derived(chipView(selectedItems, maxChips));
	const text = $derived(
		placeholder ?? $t(multiple ? 'ui.multiSelect.placeholder' : 'ui.select.placeholder')
	);
	const triggerName = $derived(
		multiple
			? summaryName(
					label,
					selectedItems.map((s) => s.label),
					maxChips,
					(key, vals) => $t(key, { values: vals })
				)
			: undefined
	);
	const activeId = $derived(
		open && activeIndex >= 0 && filtered.length > 0 ? optionId(listboxId, activeIndex) : undefined
	);

	function isSelected(option: SelectOption<string>): boolean {
		if (multiple) return values.includes(option.value as V);
		return (values[0] ?? (allowEmpty ? NONE : undefined)) === option.value;
	}

	async function load() {
		if (Array.isArray(options)) return;
		const token = ++loadToken;
		loadStatus = 'loading';
		try {
			const result = await options();
			if (token !== loadToken) return;
			loaded = result;
			loadStatus = 'ready';
		} catch {
			if (token === loadToken) loadStatus = 'failed';
		}
	}
	$effect(() => {
		if (typeof options === 'function') untrack(() => void load());
	});

	$effect(() => {
		if (!nativeOnTouch || multiple) {
			native = false;
			return;
		}
		const media = window.matchMedia('(pointer: coarse) and (max-width: 639px)');
		native = media.matches;
		const update = () => (native = media.matches);
		media.addEventListener('change', update);
		return () => media.removeEventListener('change', update);
	});

	function openMenu(viaKeyboard: boolean) {
		if (disabled || readonly || open) return;
		query = '';
		keyboard = viaKeyboard;
		activeIndex = initialActive(items, (i) => isSelected(items[i]));
		menuWidth = Math.min(360, Math.max(220, box?.offsetWidth ?? 220));
		open = true;
	}
	function closeMenu() {
		if (!open) return;
		open = false;
		query = '';
		trigger?.focus();
	}

	function commit(next: V[]) {
		onvalues(next);
	}
	function pickAt(index: number, close: boolean) {
		const option = filtered[index];
		if (!option || option.disabled) return;
		if (multiple) {
			commit(toggleValue(values, option.value as V));
			marked = null;
			if (close) closeMenu();
			return;
		}
		commit(option.value === NONE ? [] : [option.value as V]);
		closeMenu();
	}
	function clearAll() {
		if (values.length === 0) return;
		commit([]);
		marked = null;
		announcement = $t('ui.multiSelect.clearedAll');
	}

	function run(action: KeyAction, event: KeyboardEvent) {
		if (!action) return;
		switch (action.type) {
			case 'open':
				event.preventDefault();
				openMenu(true);
				return;
			case 'move': {
				event.preventDefault();
				keyboard = true;
				activeIndex = moveActive(action.key, activeIndex, filtered.length) ?? activeIndex;
				return;
			}
			case 'commit':
				event.preventDefault();
				pickAt(activeIndex, true);
				return;
			case 'toggle':
				event.preventDefault();
				pickAt(activeIndex, false);
				return;
			case 'toggle-close':
				event.preventDefault();
				pickAt(activeIndex, true);
				if (open) closeMenu();
				return;
			case 'close':
				event.preventDefault();
				closeMenu();
				return;
			case 'tab':
				// No change; focus goes back to the trigger and Tab then moves on from there.
				closeMenu();
				return;
			case 'typeahead': {
				event.preventDefault();
				if (!open) openMenu(true);
				const result = typeahead(
					typeState,
					action.char,
					performance.now(),
					filtered.map((o) => o.label),
					activeIndex
				);
				typeState = result.state;
				if (result.index !== null) {
					keyboard = true;
					activeIndex = result.index;
				}
				return;
			}
		}
	}

	function onTriggerKeydown(event: KeyboardEvent) {
		if (disabled) return;
		if (multiple && event.key === 'Backspace' && !event.ctrlKey && !event.metaKey) {
			event.preventDefault();
			const step = backspace(values, marked);
			marked = step.marked;
			if (step.removed !== null) {
				commit(step.values);
				announcement = $t('ui.multiSelect.removed', { values: { label: labelOf(step.removed) } });
			}
			return;
		}
		marked = null;
		run(
			keyAction({
				key: event.key,
				altKey: event.altKey,
				ctrlKey: event.ctrlKey,
				metaKey: event.metaKey,
				open,
				multiple,
				searching: false
			}),
			event
		);
	}
	function onSearchKeydown(event: KeyboardEvent) {
		run(
			keyAction({
				key: event.key,
				altKey: event.altKey,
				ctrlKey: event.ctrlKey,
				metaKey: event.metaKey,
				open: true,
				multiple,
				searching: true,
				emptyQuery: query === ''
			}),
			event
		);
	}

	// Typing in the search field moves the active option to the first match.
	let lastQuery = '';
	$effect(() => {
		const q = query;
		if (q === lastQuery) return;
		lastQuery = q;
		untrack(() => {
			if (open) activeIndex = filtered.length > 0 ? 0 : -1;
		});
	});
</script>

<Field
	{id}
	{label}
	{labelHidden}
	requirement={required ? 'required' : 'optional'}
	{marking}
	helper={disabled && disabledReason ? disabledReason : helper}
	{error}
>
	{#snippet children({ describedBy, invalid, required: isRequired })}
		{#if native}
			<div class="box" class:compact class:invalid>
				<select
					{id}
					class="native"
					{disabled}
					aria-invalid={invalid || undefined}
					aria-required={isRequired || undefined}
					aria-describedby={describedBy}
					value={values[0] ?? ''}
					onchange={(e) => {
						const v = e.currentTarget.value;
						commit(v === '' ? [] : [v as V]);
					}}
				>
					{#if !allowEmpty}<option value="" disabled hidden>{text}</option>{/if}
					{#if allowEmpty}<option value="">{$t('ui.select.none')}</option>{/if}
					{#each source as option (option.value)}
						<option value={option.value} disabled={option.disabled}>{option.label}</option>
					{/each}
				</select>
				<ChevronDown class="chevron" size={16} aria-hidden="true" />
			</div>
		{:else if readonly}
			<div
				{id}
				class="box readonly"
				class:compact
				role="textbox"
				tabindex="0"
				aria-readonly="true"
				aria-labelledby="{id}-label"
				aria-describedby={describedBy}
			>
				<span class="value"
					>{multiple ? selectedItems.map((s) => s.label).join(', ') : singleLabel}</span
				>
			</div>
		{:else}
			<div
				bind:this={box}
				class="box"
				class:compact
				class:invalid
				class:disabled
				class:open
				data-ui={multiple ? 'multi-select' : 'select'}
				data-testid={testId}
			>
				<button
					{id}
					bind:this={trigger}
					type="button"
					role="combobox"
					class="trigger"
					{disabled}
					aria-haspopup="listbox"
					aria-expanded={open}
					aria-controls={open ? listboxId : undefined}
					aria-activedescendant={open && !hasSearch ? activeId : undefined}
					aria-labelledby={multiple ? undefined : `${id}-label ${id}-value`}
					aria-label={triggerName}
					aria-invalid={invalid || undefined}
					aria-required={isRequired || undefined}
					aria-describedby={describedBy}
					onclick={() => (open ? closeMenu() : openMenu(false))}
					onkeydown={onTriggerKeydown}
					onblur={() => (marked = null)}
				>
					{#if multiple}
						{#if selectedItems.length === 0}
							<span class="value placeholder">{text}</span>
						{:else}
							<span class="chips">
								{#each view.chips as chip (chip.value)}
									<span class="chip" class:marked={marked === chip.value} title={chip.label}>
										<span class="chip-text">{chip.label}</span>
									</span>
								{/each}
								{#if view.hidden.length > 0}
									<span
										class="more"
										class:marked={marked !== null && view.hidden.some((h) => h.value === marked)}
										title={view.hidden.map((h) => h.label).join(', ')}
									>
										{$t('ui.multiSelect.more', { values: { n: view.hidden.length } })}
									</span>
								{/if}
							</span>
						{/if}
					{:else}
						<span
							class="value"
							class:placeholder={!singleLabel}
							id="{id}-value"
							title={singleLabel || undefined}>{singleLabel || text}</span
						>
					{/if}
				</button>
				{#if multiple && values.length > 0 && !disabled}
					<button
						type="button"
						class="clear-all"
						aria-label={$t('ui.multiSelect.clearAll')}
						onclick={clearAll}
					>
						<X size={14} aria-hidden="true" />
					</button>
				{/if}
				<ChevronDown class="chevron" size={16} aria-hidden="true" />
			</div>
			<Popover
				{open}
				anchor={box}
				onclose={closeMenu}
				role="none"
				padding="menu"
				width={menuWidth}
				autofocus={hasSearch}
				initialFocus="input"
				testId={testId ? `${testId}-menu` : undefined}
			>
				<Listbox
					id={listboxId}
					{label}
					options={filtered}
					{activeIndex}
					{multiple}
					{isSelected}
					searchable={hasSearch}
					bind:query
					bind:searchEl
					{status}
					{compact}
					{keyboard}
					onsearchkey={onSearchKeydown}
					onpick={(i) => pickAt(i, false)}
					onhover={(i) => {
						keyboard = false;
						activeIndex = i;
					}}
					onretry={() => void load()}
				>
					{#snippet footer()}
						{#if multiple}
							<div class="footer">
								<span class="count"
									>{$t('ui.multiSelect.selected', { values: { n: values.length } })}</span
								>
								<button
									type="button"
									class="clear-text"
									onpointerdown={(e) => e.preventDefault()}
									onclick={clearAll}
								>
									{$t('ui.multiSelect.clear')}
								</button>
							</div>
						{/if}
					{/snippet}
				</Listbox>
			</Popover>
		{/if}
		<span class="sr-only" role="status" aria-live="polite">{announcement}</span>
	{/snippet}
</Field>

<style>
	.box {
		position: relative;
		display: flex;
		align-items: center;
		box-sizing: border-box;
		min-height: var(--rf-field-height);
		border: var(--rf-border-width) solid var(--rf-input-border);
		border-radius: var(--rf-radius-input);
		background: var(--rf-input);
		color: var(--rf-text);
	}
	.box.compact {
		min-height: 28px;
	}
	.box:has(.trigger:focus-visible),
	.box:has(.native:focus-visible),
	.box.open,
	.box.readonly:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.box.invalid {
		background: var(--rf-danger-bg);
		border-color: var(--rf-danger);
	}
	.box.disabled {
		background: var(--rf-surface);
		border-color: var(--rf-line-strong);
		color: var(--rf-control);
	}
	.box.readonly {
		background: var(--rf-surface);
		border-color: transparent;
		padding: 0 var(--rf-space-5);
	}
	.trigger,
	.native {
		flex: 1;
		min-width: 0;
		align-self: stretch;
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		padding: 0 calc(var(--rf-space-5) + 16px + var(--rf-space-3)) 0 var(--rf-space-5);
		border: 0;
		outline: 0;
		background: transparent;
		color: inherit;
		font: inherit;
		font-size: var(--rf-size-body);
		text-align: start;
		cursor: pointer;
	}
	.compact .trigger,
	.compact .native {
		font-size: var(--rf-size-secondary);
	}
	.native {
		appearance: none;
	}
	.trigger:disabled {
		cursor: not-allowed;
	}
	.box :global(.chevron) {
		position: absolute;
		inset-inline-end: var(--rf-space-5);
		color: var(--rf-control);
		pointer-events: none;
	}
	.value {
		flex: 1;
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.value.placeholder {
		color: var(--rf-secondary);
	}
	.box.disabled .value {
		color: var(--rf-control);
	}
	.chips {
		display: flex;
		align-items: center;
		gap: var(--rf-space-2);
		min-width: 0;
		padding-inline-start: calc(var(--rf-space-3) - var(--rf-space-5));
		padding-inline-end: calc(var(--rf-space-5) + 14px);
	}
	.chip {
		display: inline-flex;
		align-items: center;
		max-width: 160px;
		height: 24px;
		padding: 0 var(--rf-space-3);
		border-radius: var(--rf-radius-chip);
		background: var(--rf-surface);
		color: var(--rf-text);
		font-size: var(--rf-size-meta);
	}
	.chip.marked,
	.more.marked {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: 0;
	}
	.chip-text {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.more {
		flex: none;
		border-radius: var(--rf-radius-chip);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
	}
	.clear-all {
		position: absolute;
		inset-inline-end: calc(var(--rf-space-5) + 16px + var(--rf-space-2));
		display: inline-grid;
		place-items: center;
		width: 24px;
		height: 24px;
		padding: 0;
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-secondary);
		cursor: pointer;
	}
	.clear-all:hover {
		background: var(--rf-hover);
	}
	.clear-all:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.footer {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
		margin-top: var(--rf-space-2);
		padding: var(--rf-space-3) var(--rf-space-4) var(--rf-space-2);
		border-top: var(--rf-border-width) solid var(--rf-line);
	}
	.count {
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		font-variant-numeric: tabular-nums;
	}
	.clear-text {
		height: 24px;
		padding: 0 var(--rf-space-3);
		border: 0;
		border-radius: var(--rf-radius-button);
		background: transparent;
		color: var(--rf-text);
		font: inherit;
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-medium);
		cursor: pointer;
	}
	.clear-text:hover {
		background: var(--rf-hover);
	}
	.clear-text:focus-visible {
		outline: var(--rf-border-width-focus) solid var(--rf-focus);
		outline-offset: -1px;
	}
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		overflow: hidden;
		clip-path: inset(50%);
		white-space: nowrap;
	}
	@media (pointer: coarse) {
		.box {
			min-height: var(--rf-touch-target);
		}
	}
</style>
