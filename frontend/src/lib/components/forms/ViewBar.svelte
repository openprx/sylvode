<script lang="ts">
	/**
	 * The view bar above the record list: switcher and view menu, the quick Filter / Sort /
	 * Fields controls, the "Modified" state with Reset, and "Save view…".
	 */
	import { t } from 'svelte-i18n';
	import type { FormField, FormView } from '#lib/api/forms.js';
	import SaveViewPopover from './SaveViewPopover.svelte';
	import ViewArrangementControls, {
		type SimpleFilter,
		type SortChoice
	} from './ViewArrangementControls.svelte';
	import ViewMenu from './ViewMenu.svelte';
	import ViewSwitcher from './ViewSwitcher.svelte';
	import { viewIsDefault, viewIsPrivate } from './view-icons.js';

	interface Props {
		views: FormView[];
		/** The saved view behind the working view; `null` for the built-in table. */
		view: FormView | null;
		currentId: string;
		currentType: FormView['view_type'];
		modified: boolean;
		can: { manage: boolean; share: boolean; setDefault: boolean; create: boolean };
		busy: boolean;
		saveOpen: boolean;
		saveError: string | null;
		suggestedName: string;
		fields: FormField[];
		sortableKeys: string[];
		filters: SimpleFilter[];
		filterLogic: 'all' | 'any';
		advancedFilters: boolean;
		sort: SortChoice | null;
		columns: string[];
		hiddenByPermission: Set<string>;
		partialNotice: string | null;
		onselect: (viewId: string) => void;
		oncreate: (type: FormView['view_type']) => void;
		onupdate: () => void;
		onrename: (name: string) => void;
		onvisibility: (visibility: 'shared' | 'private') => void;
		ondefault: (on: boolean) => void;
		onlayout: () => void;
		onduplicate: () => void;
		onreset: () => void;
		ondelete: () => void;
		onsaveopen: (open: boolean) => void;
		onsave: (input: { name: string; visibility: 'private' | 'shared' }) => void;
		onfilters: (filters: SimpleFilter[], logic: 'all' | 'any') => void;
		onsort: (sort: SortChoice | null) => void;
		oncolumns: (columns: string[]) => void;
	}

	let props: Props = $props();
	let saveButton = $state<HTMLButtonElement | null>(null);

	const defaultId = $derived(props.views.find((view) => viewIsDefault(view))?.id ?? null);
</script>

<div
	class="flex flex-wrap items-center gap-x-1 gap-y-2 border-b border-slate-200 px-3 py-2 dark:border-slate-700"
	data-view-bar
>
	<ViewSwitcher
		views={props.views}
		currentId={props.currentId}
		currentType={props.currentType}
		{defaultId}
		modified={props.modified}
		canCreate={props.can.create}
		onselect={props.onselect}
		oncreate={props.oncreate}
	/>
	<ViewMenu
		view={props.view}
		modified={props.modified}
		can={props.can}
		isDefault={viewIsDefault(props.view)}
		isPrivate={viewIsPrivate(props.view)}
		busy={props.busy}
		onupdate={props.onupdate}
		onrename={props.onrename}
		onvisibility={props.onvisibility}
		ondefault={props.ondefault}
		onlayout={props.onlayout}
		onduplicate={props.onduplicate}
		onreset={props.onreset}
		ondelete={props.ondelete}
	/>
	<span
		class="mx-1 hidden h-5 w-px bg-slate-200 sm:inline-block dark:bg-slate-700"
		aria-hidden="true"
	></span>
	<ViewArrangementControls
		fields={props.fields}
		sortableKeys={props.sortableKeys}
		filters={props.filters}
		filterLogic={props.filterLogic}
		advanced={props.advancedFilters}
		sort={props.sort}
		columns={props.columns}
		hiddenByPermission={props.hiddenByPermission}
		onfilters={props.onfilters}
		onsort={props.onsort}
		oncolumns={props.oncolumns}
		onadvanced={props.onlayout}
	/>
	<span class="flex-1"></span>
	{#if props.modified}
		<span
			class="inline-flex items-center gap-1.5 text-xs font-medium text-amber-700 dark:text-amber-300"
			data-view-modified
			><span class="h-1.5 w-1.5 rounded-full bg-amber-500" aria-hidden="true"></span>{$t(
				'forms.views.modified'
			)}</span
		>
		<button
			type="button"
			class="h-8 rounded-md px-2 text-sm text-slate-700 hover:bg-slate-100 focus-visible:outline-2 focus-visible:outline-blue-500 dark:text-slate-200 dark:hover:bg-slate-800"
			data-view-reset
			onclick={props.onreset}>{$t('forms.views.reset')}</button
		>
	{/if}
	{#if props.can.create}
		<button
			type="button"
			bind:this={saveButton}
			class="h-8 rounded-md px-3 text-sm font-medium focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-blue-500 {props.modified
				? 'bg-blue-600 text-white hover:bg-blue-700'
				: 'border border-slate-300 text-slate-700 hover:bg-slate-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800'}"
			aria-haspopup="dialog"
			aria-expanded={props.saveOpen}
			data-view-save-as
			onclick={() => props.onsaveopen(!props.saveOpen)}>{$t('forms.views.saveViewAction')}</button
		>
		<SaveViewPopover
			open={props.saveOpen}
			anchor={saveButton}
			suggestedName={props.suggestedName}
			canShare={props.can.share}
			existingNames={props.views.map((view) => view.name)}
			saving={props.busy}
			error={props.saveError}
			onsave={props.onsave}
			oncancel={() => props.onsaveopen(false)}
		/>
	{/if}
</div>
{#if props.partialNotice}
	<p
		class="border-b border-slate-200 bg-slate-50 px-4 py-1.5 text-xs text-slate-600 dark:border-slate-700 dark:bg-slate-800/50 dark:text-slate-300"
		data-view-partial-filter
	>
		{props.partialNotice}
	</p>
{/if}
