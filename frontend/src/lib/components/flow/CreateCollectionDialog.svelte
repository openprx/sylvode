<script lang="ts">
	// Standalone Collection create dialog (`ui-surface-v1.md` v0.6: "Navigator 的 standalone
	// Collection 创建走通用 object create"). Intent, key and id handling live in
	// `$lib/flow/create-collection`; this component only holds the form fields.
	import { t } from 'svelte-i18n';
	import { get } from 'svelte/store';
	import Modal from '$lib/components/Modal.svelte';
	import type { AcceptedChange } from '$lib/api/flow';
	import { FlowCommandService } from '$lib/flow/command-service';
	import {
		COLLECTION_TITLE_MAX_CHARS,
		COLLECTION_VIEW_NAME_KEYS,
		COLLECTION_VIEW_TYPES,
		CreateCollectionSession,
		type CollectionDraft,
		type CollectionViewType
	} from '$lib/flow/create-collection';

	interface Props {
		open: boolean;
		workspaceId: string;
		oncreated: (change: AcceptedChange) => void;
		onclose: () => void;
	}

	let { open, workspaceId, oncreated, onclose }: Props = $props();

	const VALIDATION_KEYS = {
		title_required: 'flow.nav.createCollection.titleRequired',
		title_too_long: 'flow.nav.createCollection.tooLong',
		field_label_required: 'flow.nav.createCollection.fieldLabelRequired'
	} as const;

	// Recreated per opening so a closed dialog's intent (and key) never leaks into the next one.
	let session = $state<CreateCollectionSession | null>(null);
	let title = $state('');
	let fieldLabels = $state<string[]>([]);
	let viewType = $state<CollectionViewType>('table');
	let submitting = $state(false);
	let errorKey = $state<string | null>(null);
	let errorRetryable = $state(false);
	let wasOpen = false;

	$effect(() => {
		if (open && !wasOpen) {
			session = new CreateCollectionSession(workspaceId, new FlowCommandService());
			title = '';
			fieldLabels = [get(t)('flow.nav.createCollection.defaultFieldLabel')];
			viewType = 'table';
			errorKey = null;
			errorRetryable = false;
		}
		wasOpen = open;
	});

	function draft(): CollectionDraft {
		return {
			title,
			fieldLabels,
			viewType,
			viewName: get(t)(COLLECTION_VIEW_NAME_KEYS[viewType])
		};
	}

	function addField(): void {
		fieldLabels = [...fieldLabels, ''];
		errorKey = null;
	}

	function removeField(index: number): void {
		fieldLabels = fieldLabels.filter((_, i) => i !== index);
		errorKey = null;
	}

	async function submit(event?: SubmitEvent): Promise<void> {
		event?.preventDefault();
		if (!session || submitting) return;
		submitting = true;
		errorKey = null;
		try {
			const outcome = await session.submit(draft());
			if (outcome.kind === 'created') {
				oncreated(outcome.change);
				return;
			}
			if (outcome.kind === 'invalid') {
				errorKey = VALIDATION_KEYS[outcome.reason];
				errorRetryable = true;
				return;
			}
			errorKey = outcome.key;
			errorRetryable = outcome.retryable;
		} finally {
			submitting = false;
		}
	}
</script>

<Modal {open} title={$t('flow.nav.createCollection.title')} {onclose}>
	<form
		id="flow-create-collection-form"
		class="space-y-4 text-sm text-slate-700 dark:text-slate-300"
		data-testid="flow-create-collection"
		onsubmit={submit}
		oninput={() => (errorKey = null)}
	>
		<label class="block">
			<span class="mb-1 block font-medium">{$t('flow.nav.createCollection.nameLabel')}</span>
			<input
				type="text"
				bind:value={title}
				maxlength={COLLECTION_TITLE_MAX_CHARS}
				class="min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
				data-testid="flow-create-collection-name"
			/>
		</label>

		<fieldset>
			<legend class="mb-1 font-medium">{$t('flow.nav.createCollection.fieldsLabel')}</legend>
			<p class="mb-2 text-xs text-slate-500 dark:text-slate-400">
				{$t('flow.nav.createCollection.fieldsHint')}
			</p>
			{#if fieldLabels.length === 0}
				<p class="mb-2 text-xs text-slate-500 dark:text-slate-400">
					{$t('flow.nav.createCollection.noFields')}
				</p>
			{/if}
			<ul class="space-y-2">
				{#each fieldLabels, index}
					<li class="flex items-center gap-2">
						<input
							type="text"
							bind:value={fieldLabels[index]}
							maxlength={COLLECTION_TITLE_MAX_CHARS}
							aria-label={$t('flow.nav.createCollection.fieldLabel', {
								values: { position: index + 1 }
							})}
							class="min-h-11 flex-1 rounded-md border border-slate-300 bg-white px-3 text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
							data-testid="flow-create-collection-field"
						/>
						<span class="text-xs text-slate-500 dark:text-slate-400"
							>{$t('flow.nav.createCollection.fieldTypeText')}</span
						>
						<button
							type="button"
							class="min-h-11 min-w-11 rounded-md px-2 text-slate-600 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-300 dark:hover:bg-slate-700"
							onclick={() => removeField(index)}
							aria-label={$t('flow.nav.createCollection.removeField', {
								values: { position: index + 1 }
							})}
						>
							{$t('flow.nav.createCollection.remove')}
						</button>
					</li>
				{/each}
			</ul>
			<button
				type="button"
				class="mt-2 min-h-11 rounded-md px-3 font-medium text-blue-700 hover:bg-blue-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300 dark:hover:bg-slate-700"
				onclick={addField}
				data-testid="flow-create-collection-add-field"
			>
				{$t('flow.nav.createCollection.addField')}
			</button>
		</fieldset>

		<fieldset>
			<legend class="mb-1 font-medium">{$t('flow.nav.createCollection.viewLabel')}</legend>
			<div class="flex gap-4">
				{#each COLLECTION_VIEW_TYPES as type (type)}
					<label class="flex min-h-11 items-center gap-2">
						<input
							type="radio"
							name="flow-create-collection-view"
							value={type}
							bind:group={viewType}
							data-testid={`flow-create-collection-view-${type}`}
						/>
						<span>{$t(COLLECTION_VIEW_NAME_KEYS[type])}</span>
					</label>
				{/each}
			</div>
		</fieldset>

		<div aria-live="polite">
			{#if errorKey}
				<p
					class="text-red-600 dark:text-red-400"
					role="alert"
					data-testid="flow-create-collection-error"
				>
					{$t(errorKey)}
					{#if !errorRetryable}
						{$t('flow.nav.createCollection.notRetryable')}
					{/if}
				</p>
			{/if}
		</div>
	</form>
	{#snippet footer()}
		<button
			type="button"
			class="min-h-11 rounded-md px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-500 dark:text-slate-200 dark:hover:bg-slate-700"
			onclick={onclose}
		>
			{$t('common.cancel')}
		</button>
		<button
			type="submit"
			form="flow-create-collection-form"
			class="min-h-11 rounded-md bg-blue-600 px-4 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
			disabled={submitting || (errorKey !== null && !errorRetryable)}
			data-testid="flow-create-collection-submit"
		>
			{submitting
				? $t('flow.nav.createCollection.creating')
				: $t('flow.nav.createCollection.submit')}
		</button>
	{/snippet}
</Modal>
