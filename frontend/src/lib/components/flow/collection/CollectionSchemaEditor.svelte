<script lang="ts">
	// Schema design mode (`versions/v0.6-collections.md`: "schema design 是显式模式"): field
	// create / rename / type / options / archive / reorder and view create / rename / reorder.
	// Rendered only while the mode is `schema`, and the controller refuses every `field_*` /
	// `view_*` command outside it anyway.
	import { t } from 'svelte-i18n';
	import type { FlowCollectionFieldType } from '$lib/api/flow';
	import {
		CollectionViewController,
		type CollectionViewState,
		type SchemaOutcome
	} from '$lib/flow/collection-view';
	import {
		COLLECTION_FIELD_TYPES,
		CREATABLE_FIELD_TYPES,
		type CollectionField
	} from '$lib/flow/collection-values';

	interface Props {
		controller: CollectionViewController;
		snapshot: CollectionViewState;
	}

	let { controller, snapshot }: Props = $props();

	const FIELD_TYPE_KEYS: Readonly<Record<FlowCollectionFieldType, string>> = {
		text: 'flow.collection.fieldType.text',
		number: 'flow.collection.fieldType.number',
		boolean: 'flow.collection.fieldType.boolean',
		date: 'flow.collection.fieldType.date',
		select: 'flow.collection.fieldType.select',
		multi_select: 'flow.collection.fieldType.multi_select',
		relation: 'flow.collection.fieldType.relation'
	};

	const fields = $derived(snapshot.collection?.fields ?? []);
	const views = $derived(snapshot.collection?.views ?? []);

	let newLabel = $state('');
	let newType = $state<FlowCollectionFieldType>('text');
	let newOptions = $state('');
	let newViewName = $state('');
	let newViewKind = $state<'table' | 'board'>('table');
	let message = $state<{ key: string; code: string | null } | null>(null);
	let labels = $state<Record<string, string>>({});
	let optionText = $state<Record<string, string>>({});
	let viewNames = $state<Record<string, string>>({});

	function splitOptions(raw: string): string[] {
		return raw
			.split(',')
			.map((part) => part.trim())
			.filter((part) => part !== '');
	}

	function report(outcome: SchemaOutcome): boolean {
		message = outcome.ok ? null : { key: outcome.key, code: outcome.code };
		return outcome.ok;
	}

	async function addField(): Promise<void> {
		const ok = report(await controller.createField(newLabel, newType, splitOptions(newOptions)));
		if (ok) {
			newLabel = '';
			newOptions = '';
		}
	}

	async function rename(field: CollectionField): Promise<void> {
		const label = labels[field.fieldId];
		if (label === undefined || label.trim() === field.label) return;
		if (report(await controller.renameField(field.fieldId, label))) delete labels[field.fieldId];
	}

	async function saveOptions(field: CollectionField): Promise<void> {
		const raw = optionText[field.fieldId];
		if (raw === undefined) return;
		if (report(await controller.setFieldOptions(field.fieldId, splitOptions(raw)))) {
			delete optionText[field.fieldId];
		}
	}

	async function changeType(field: CollectionField, type: string): Promise<void> {
		if (type === field.type) return;
		report(await controller.changeFieldType(field.fieldId, type as FlowCollectionFieldType));
	}

	async function addView(): Promise<void> {
		if (report(await controller.createView(newViewName, newViewKind))) newViewName = '';
	}

	async function renameView(viewId: string, current: string): Promise<void> {
		const name = viewNames[viewId];
		if (name === undefined || name.trim() === current) return;
		if (report(await controller.renameView(viewId, name))) delete viewNames[viewId];
	}

	const inputClass =
		'min-h-11 rounded border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100';
	const iconButton =
		'min-h-11 min-w-11 rounded text-sm hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-30 dark:hover:bg-slate-800';
</script>

<section
	class="space-y-4 border-b border-amber-200 bg-amber-50/60 px-4 py-3 dark:border-amber-900 dark:bg-amber-950/30"
	aria-label={$t('flow.collection.schema.title')}
	data-testid="collection-schema-editor"
>
	{#if message}
		<p
			class="text-sm text-red-700 dark:text-red-300"
			role="alert"
			data-error-code={message.code ?? ''}
		>
			{$t(message.key)}
		</p>
	{/if}

	<div class="space-y-2">
		<h3 class="text-sm font-semibold text-slate-800 dark:text-slate-100">
			{$t('flow.collection.schema.fields')}
		</h3>
		<ul class="space-y-2">
			{#each fields as field, index (field.fieldId)}
				<li
					class="flex flex-wrap items-center gap-2"
					data-testid="collection-schema-field"
					data-field-id={field.fieldId}
				>
					<input
						class="{inputClass} w-44"
						value={labels[field.fieldId] ?? field.label}
						aria-label={$t('flow.collection.schema.fieldLabel')}
						oninput={(event) => (labels[field.fieldId] = event.currentTarget.value)}
						onkeydown={(event) => {
							if (event.key === 'Enter') void rename(field);
						}}
						onblur={() => void rename(field)}
					/>
					<select
						class={inputClass}
						value={field.type ?? ''}
						aria-label={$t('flow.collection.schema.fieldType')}
						disabled={snapshot.schemaBusy}
						onchange={(event) => void changeType(field, event.currentTarget.value)}
					>
						{#each COLLECTION_FIELD_TYPES as type (type)}
							<option value={type}>{$t(FIELD_TYPE_KEYS[type])}</option>
						{/each}
					</select>
					{#if field.type === 'select' || field.type === 'multi_select'}
						<input
							class="{inputClass} w-56"
							value={optionText[field.fieldId] ?? field.options.join(', ')}
							aria-label={$t('flow.collection.schema.options')}
							placeholder={$t('flow.collection.schema.optionsPlaceholder')}
							oninput={(event) => (optionText[field.fieldId] = event.currentTarget.value)}
						/>
						<button
							type="button"
							class="min-h-11 rounded border border-slate-300 px-3 text-sm dark:border-slate-600"
							disabled={optionText[field.fieldId] === undefined || snapshot.schemaBusy}
							onclick={() => void saveOptions(field)}
							>{$t('flow.collection.schema.saveOptions')}</button
						>
					{/if}
					<button
						type="button"
						class={iconButton}
						aria-label={$t('flow.collection.schema.moveUp')}
						disabled={index === 0 || snapshot.schemaBusy}
						onclick={async () => report(await controller.moveField(field.fieldId, -1))}>↑</button
					>
					<button
						type="button"
						class={iconButton}
						aria-label={$t('flow.collection.schema.moveDown')}
						disabled={index === fields.length - 1 || snapshot.schemaBusy}
						onclick={async () => report(await controller.moveField(field.fieldId, 1))}>↓</button
					>
					<button
						type="button"
						class="min-h-11 rounded px-3 text-sm text-red-700 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950"
						disabled={snapshot.schemaBusy}
						onclick={async () => report(await controller.archiveField(field.fieldId))}
						>{$t('flow.collection.schema.archive')}</button
					>
				</li>
			{/each}
		</ul>
		<form
			class="flex flex-wrap items-end gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				void addField();
			}}
		>
			<label class="flex flex-col text-xs text-slate-600 dark:text-slate-300">
				{$t('flow.collection.schema.newFieldLabel')}
				<input
					class="{inputClass} w-44"
					bind:value={newLabel}
					data-testid="collection-new-field-label"
				/>
			</label>
			<label class="flex flex-col text-xs text-slate-600 dark:text-slate-300">
				{$t('flow.collection.schema.fieldType')}
				<select class={inputClass} bind:value={newType} data-testid="collection-new-field-type">
					{#each CREATABLE_FIELD_TYPES as type (type)}
						<option value={type}>{$t(FIELD_TYPE_KEYS[type])}</option>
					{/each}
				</select>
			</label>
			{#if newType === 'select' || newType === 'multi_select'}
				<label class="flex flex-col text-xs text-slate-600 dark:text-slate-300">
					{$t('flow.collection.schema.options')}
					<input
						class="{inputClass} w-56"
						bind:value={newOptions}
						placeholder={$t('flow.collection.schema.optionsPlaceholder')}
						data-testid="collection-new-field-options"
					/>
				</label>
			{/if}
			<button
				type="submit"
				class="min-h-11 rounded bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 disabled:opacity-50"
				disabled={snapshot.schemaBusy}
				data-testid="collection-add-field">{$t('flow.collection.schema.addField')}</button
			>
		</form>
	</div>

	<div class="space-y-2">
		<h3 class="text-sm font-semibold text-slate-800 dark:text-slate-100">
			{$t('flow.collection.schema.views')}
		</h3>
		<ul class="space-y-2">
			{#each views as view, index (view.viewId)}
				<li class="flex flex-wrap items-center gap-2" data-testid="collection-schema-view">
					<input
						class="{inputClass} w-44"
						value={viewNames[view.viewId] ?? view.name}
						aria-label={$t('flow.collection.schema.viewName')}
						oninput={(event) => (viewNames[view.viewId] = event.currentTarget.value)}
						onkeydown={(event) => {
							if (event.key === 'Enter') void renameView(view.viewId, view.name);
						}}
						onblur={() => void renameView(view.viewId, view.name)}
					/>
					<span class="text-xs text-slate-500">
						{view.kind === 'board'
							? $t('flow.collection.view.board')
							: $t('flow.collection.view.table')}
					</span>
					<button
						type="button"
						class={iconButton}
						aria-label={$t('flow.collection.schema.moveUp')}
						disabled={index === 0 || snapshot.schemaBusy}
						onclick={async () => report(await controller.moveView(view.viewId, -1))}>↑</button
					>
					<button
						type="button"
						class={iconButton}
						aria-label={$t('flow.collection.schema.moveDown')}
						disabled={index === views.length - 1 || snapshot.schemaBusy}
						onclick={async () => report(await controller.moveView(view.viewId, 1))}>↓</button
					>
				</li>
			{/each}
		</ul>
		<form
			class="flex flex-wrap items-end gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				void addView();
			}}
		>
			<label class="flex flex-col text-xs text-slate-600 dark:text-slate-300">
				{$t('flow.collection.schema.newViewName')}
				<input class="{inputClass} w-44" bind:value={newViewName} />
			</label>
			<label class="flex flex-col text-xs text-slate-600 dark:text-slate-300">
				{$t('flow.collection.schema.viewKind')}
				<select class={inputClass} bind:value={newViewKind}>
					<option value="table">{$t('flow.collection.view.table')}</option>
					<option value="board">{$t('flow.collection.view.board')}</option>
				</select>
			</label>
			<button
				type="submit"
				class="min-h-11 rounded border border-slate-300 px-3 text-sm font-medium dark:border-slate-600"
				disabled={snapshot.schemaBusy}>{$t('flow.collection.schema.addView')}</button
			>
		</form>
	</div>
</section>
