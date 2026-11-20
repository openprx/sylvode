<script lang="ts">
	/**
	 * The view switcher: a menu button opening a listbox of "All records", then shared views and
	 * the user's own private views. Choosing one is an explicit choice that the page remembers.
	 * "New view…" offers the view types and starts an unsaved working view of that type.
	 */
	import { tick } from 'svelte';
	import { t } from 'svelte-i18n';
	import { Check, ChevronDown, Lock, Plus } from '@lucide/svelte';
	import type { FormView } from '#lib/api/forms.js';
	import { ALL_RECORDS_VIEW_ID } from '#lib/forms/view-precedence.js';
	import Popover from './Popover.svelte';
	import { CREATABLE_VIEW_TYPES, VIEW_TYPE_ICONS, viewIsPrivate } from './view-icons.js';

	interface Props {
		views: FormView[];
		currentId: string;
		/** Type of the working view (may differ from the saved one while modified). */
		currentType: FormView['view_type'];
		defaultId: string | null;
		modified: boolean;
		canCreate: boolean;
		onselect: (viewId: string) => void;
		oncreate: (type: FormView['view_type']) => void;
	}

	let { views, currentId, currentType, defaultId, modified, canCreate, onselect, oncreate }: Props =
		$props();

	let open = $state(false);
	let creating = $state(false);
	let button = $state<HTMLButtonElement | null>(null);
	let listbox = $state<HTMLElement | null>(null);
	let query = $state('');
	let activeIndex = $state(0);

	const shared = $derived(views.filter((view) => !viewIsPrivate(view)));
	const mine = $derived(views.filter((view) => viewIsPrivate(view)));
	const filterable = $derived(views.length > 7);
	const needle = $derived(query.trim().toLowerCase());
	const matches = (name: string) => !needle || name.toLowerCase().includes(needle);
	const allLabel = $derived($t('forms.views.allRecords'));
	const options = $derived([
		...(matches(allLabel) ? [{ id: ALL_RECORDS_VIEW_ID, name: allLabel }] : []),
		...shared.filter((view) => matches(view.name)),
		...mine.filter((view) => matches(view.name))
	]);
	const groups = $derived(
		[
			{
				key: 'all',
				label: '',
				items: options.filter((option) => option.id === ALL_RECORDS_VIEW_ID)
			},
			{
				key: 'shared',
				label: $t('forms.views.sharedGroup'),
				items: shared.filter((view) => matches(view.name))
			},
			{
				key: 'mine',
				label: $t('forms.views.privateGroup'),
				items: mine.filter((view) => matches(view.name))
			}
		].filter((group) => group.items.length > 0)
	);
	const current = $derived(views.find((view) => view.id === currentId) ?? null);
	const currentName = $derived(current?.name ?? allLabel);
	const CurrentIcon = $derived(VIEW_TYPE_ICONS[currentType] ?? VIEW_TYPE_ICONS.grid);

	function optionId(id: string): string {
		return `view-option-${id}`;
	}

	async function show() {
		open = true;
		creating = false;
		query = '';
		activeIndex = Math.max(
			0,
			options.findIndex((option) => option.id === currentId)
		);
		await tick();
		await tick();
		(filterable
			? listbox?.parentElement?.querySelector<HTMLInputElement>('[data-view-switcher-filter]')
			: listbox
		)?.focus();
	}

	function close() {
		open = false;
		creating = false;
	}

	function choose(id: string) {
		close();
		button?.focus();
		onselect(id);
	}

	function onListKey(event: KeyboardEvent) {
		const count = options.length;
		if (count === 0) return;
		if (event.key === 'ArrowDown') activeIndex = (activeIndex + 1) % count;
		else if (event.key === 'ArrowUp') activeIndex = (activeIndex - 1 + count) % count;
		else if (event.key === 'Home') activeIndex = 0;
		else if (event.key === 'End') activeIndex = count - 1;
		else if (event.key === 'Enter' || event.key === ' ') {
			const option = options[activeIndex];
			if (option) choose(option.id);
		} else if (event.key.length === 1 && !filterable && /\S/.test(event.key)) {
			// Type-ahead: jump to the next option starting with the typed character.
			const start = (activeIndex + 1) % count;
			for (let offset = 0; offset < count; offset += 1) {
				const index = (start + offset) % count;
				if (options[index].name.toLowerCase().startsWith(event.key.toLowerCase())) {
					activeIndex = index;
					break;
				}
			}
		} else return;
		event.preventDefault();
		listbox
			?.querySelector<HTMLElement>(`#${CSS.escape(optionId(options[activeIndex]?.id ?? ''))}`)
			?.scrollIntoView({ block: 'nearest' });
	}

	function onButtonKey(event: KeyboardEvent) {
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			event.preventDefault();
			void show();
		}
	}

	const optionClass =
		'flex min-h-8 w-full cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-start text-sm text-slate-700 dark:text-slate-200';
</script>

<button
	type="button"
	bind:this={button}
	class="inline-flex h-8 max-w-[18rem] items-center gap-1.5 rounded-md px-2 text-sm font-semibold text-slate-900 hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-500 dark:text-slate-100 dark:hover:bg-slate-800"
	aria-haspopup="listbox"
	aria-expanded={open}
	aria-label={`${$t('forms.views.switcherLabel')}: ${currentName}`}
	title={currentName}
	data-view-switcher={currentId}
	onclick={() => (open ? close() : void show())}
	onkeydown={onButtonKey}
>
	<CurrentIcon class="h-4 w-4 shrink-0 text-slate-500 dark:text-slate-400" aria-hidden="true" />
	<span class="max-w-[24ch] truncate">{currentName}</span>
	{#if modified}
		<span class="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" aria-hidden="true"></span>
	{/if}
	<ChevronDown class="h-3.5 w-3.5 shrink-0 text-slate-500" aria-hidden="true" />
</button>

<Popover
	{open}
	anchor={button}
	onclose={close}
	role="none"
	width={288}
	autofocus={false}
	testId="view-switcher"
>
	{#if filterable}
		<input
			type="search"
			class="mb-2 h-8 w-full rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-800"
			placeholder={$t('forms.views.filterViews')}
			aria-label={$t('forms.views.filterViews')}
			aria-controls="view-switcher-listbox"
			bind:value={query}
			oninput={() => (activeIndex = 0)}
			onkeydown={onListKey}
			data-view-switcher-filter
		/>
	{/if}
	<div
		bind:this={listbox}
		id="view-switcher-listbox"
		role="listbox"
		tabindex="0"
		aria-label={$t('forms.views.switcherLabel')}
		aria-activedescendant={options[activeIndex] ? optionId(options[activeIndex].id) : undefined}
		class="space-y-0.5 focus:outline-none"
		onkeydown={onListKey}
	>
		{#each groups as group (group.key)}
			{#if group.label}
				<div
					class="px-2 pt-2 pb-1 text-[11px] font-semibold tracking-wide text-slate-500 uppercase dark:text-slate-400"
					role="presentation"
				>
					{group.label}
				</div>
			{/if}
			{#each group.items as option (option.id)}
				{@const index = options.findIndex((item) => item.id === option.id)}
				{@const view = views.find((item) => item.id === option.id) ?? null}
				{@const Icon = view
					? (VIEW_TYPE_ICONS[view.view_type] ?? VIEW_TYPE_ICONS.grid)
					: VIEW_TYPE_ICONS.grid}
				<div
					id={optionId(option.id)}
					role="option"
					aria-selected={option.id === currentId}
					tabindex="-1"
					class="{optionClass} {index === activeIndex
						? 'bg-slate-100 dark:bg-slate-800'
						: 'hover:bg-slate-50 dark:hover:bg-slate-800/60'}"
					data-view-option={option.id}
					onclick={() => choose(option.id)}
					onmouseenter={() => (activeIndex = index)}
					onkeydown={onListKey}
				>
					<Icon class="h-4 w-4 shrink-0 text-slate-500 dark:text-slate-400" aria-hidden="true" />
					<span class="min-w-0 flex-1 truncate" title={option.name}>{option.name}</span>
					{#if view && viewIsPrivate(view)}
						<Lock class="h-3.5 w-3.5 shrink-0 text-slate-400" aria-hidden="true" />
						<span class="sr-only">{$t('forms.views.privateBadge')}</span>
					{/if}
					{#if option.id === defaultId}
						<span
							class="rounded bg-slate-100 px-1 text-[10px] font-medium text-slate-600 dark:bg-slate-700 dark:text-slate-200"
							>{$t('forms.views.defaultBadge')}</span
						>
					{/if}
					<Check
						class="h-4 w-4 shrink-0 {option.id === currentId
							? 'text-blue-600 dark:text-blue-400'
							: 'invisible'}"
						aria-hidden="true"
					/>
				</div>
			{/each}
		{/each}
	</div>
	{#if canCreate}
		<div class="mt-1 border-t border-slate-200 pt-1 dark:border-slate-700">
			{#if creating}
				<div role="group" aria-label={$t('forms.views.newView')} class="space-y-0.5">
					{#each CREATABLE_VIEW_TYPES as type (type)}
						{@const Icon = VIEW_TYPE_ICONS[type]}
						<button
							type="button"
							class="{optionClass} hover:bg-slate-100 focus-visible:bg-slate-100 focus-visible:outline-none dark:hover:bg-slate-800 dark:focus-visible:bg-slate-800"
							data-view-new-type={type}
							onclick={() => {
								close();
								button?.focus();
								oncreate(type);
							}}
						>
							<Icon class="h-4 w-4 text-slate-500" aria-hidden="true" />
							{$t(`forms.views.types.${type}`)}
						</button>
					{/each}
				</div>
			{:else}
				<button
					type="button"
					class="{optionClass} hover:bg-slate-100 focus-visible:bg-slate-100 focus-visible:outline-none dark:hover:bg-slate-800 dark:focus-visible:bg-slate-800"
					aria-expanded={creating}
					data-view-new
					onclick={async () => {
						creating = true;
						await tick();
						listbox?.parentElement?.querySelector<HTMLElement>('[data-view-new-type]')?.focus();
					}}
				>
					<Plus class="h-4 w-4 text-slate-500" aria-hidden="true" />
					{$t('forms.views.newView')}
				</button>
			{/if}
		</div>
	{/if}
</Popover>
