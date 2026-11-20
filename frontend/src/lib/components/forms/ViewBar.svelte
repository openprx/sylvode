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
		/** The view was changed by someone else since it was loaded. */
		conflict: boolean;
		/** "Changes reset" can still be undone. */
		undoAvailable: boolean;
		onselect: (viewId: string) => void;
		oncreate: (type: FormView['view_type']) => void;
		onupdate: () => void;
		onrename: (name: string) => void;
		onvisibility: (visibility: 'shared' | 'private') => void;
		ondefault: (on: boolean) => void;
		onlayout: () => void;
		onduplicate: () => void;
		onreset: () => void;
		onundo: () => void;
		onreloadview: () => void;
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
	class="flex flex-wrap items-center gap-x-1 gap-y-2 border-b border-line px-3 py-2"
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
		class="mx-1 hidden h-5 w-px bg-line sm:inline-block"
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
			class="inline-flex items-center gap-1.5 text-xs font-medium text-warning"
			data-view-modified
			><span class="h-1.5 w-1.5 rounded-full bg-warning" aria-hidden="true"></span>{$t(
				'forms.views.modified'
			)}</span
		>
		<button
			type="button"
			class="h-8 rounded-control px-2 text-sm text-fg hover:bg-surface"
			data-view-reset
			onclick={props.onreset}>{$t('forms.views.reset')}</button
		>
	{:else if props.undoAvailable}
		<span class="text-xs text-fg-muted">{$t('forms.views.resetDone')}</span>
		<button
			type="button"
			class="h-8 rounded-control px-2 text-sm font-medium text-fg hover:bg-surface"
			data-view-undo-reset
			onclick={props.onundo}>{$t('forms.views.undo')}</button
		>
	{/if}
	{#if props.can.create}
		<button
			type="button"
			bind:this={saveButton}
			class="h-8 rounded-control px-3 text-sm font-medium {props.modified
				? 'bg-primary text-on-primary'
				: 'border border-input text-fg hover:bg-surface'}"
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
{#if props.conflict}
	<div
		class="flex flex-wrap items-center gap-2 border-b border-line bg-warning-soft px-4 py-2 text-sm text-warning"
		role="alert"
		data-view-conflict
	>
		<span>{$t('forms.views.conflict')}</span>
		<button type="button" class="min-h-6 font-medium underline" onclick={props.onreloadview}
			>{$t('forms.views.reloadView')}</button
		>
		<button
			type="button"
			class="min-h-6 font-medium underline"
			onclick={() => props.onsaveopen(true)}>{$t('forms.views.saveAsNew')}</button
		>
	</div>
{/if}
{#if props.partialNotice}
	<p
		class="border-b border-line bg-surface px-4 py-1.5 text-xs text-fg-muted"
		data-view-partial-filter
	>
		{props.partialNotice}
	</p>
{/if}
