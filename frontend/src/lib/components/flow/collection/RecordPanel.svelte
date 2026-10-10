<script lang="ts">
	// `record:panel` renderer: the side panel of one record. The properties section uses the same
	// typed editors as the table; the body section edits the record's body. Its state (which
	// record, drafts, statuses) lives in `CollectionViewController`, so switching table/board or
	// closing and reopening the panel keeps uncommitted edits. Opening this panel is the only
	// thing that opens the record's document; closing it closes the document.
	//
	// The body is a plain string on the record (`collections.rs`: `record_patch {body}` writes the
	// `record:body` node's `body` property) and a Record admits no collaboration session
	// (`collab/ticket.rs::object_type_admits_collab_session`), so it is edited as text and saved
	// with `record_patch`, not mounted in the Page canvas.
	//
	// Under field secrecy the server withholds the record's whole `semantic_content`, body
	// included; the body section then says so and offers no editor (an empty text box would invite
	// overwriting a body the user cannot see). Archiving (`record_archive`) asks for confirmation
	// in place first.
	import { t } from 'svelte-i18n';
	import {
		CollectionViewController,
		draftIn,
		type CollectionViewState
	} from '$lib/flow/collection-view';
	import { editorValue, isEditableField } from '$lib/flow/collection-values';
	import CollectionCellValue from './CollectionCellValue.svelte';
	import CollectionValueEditor from './CollectionValueEditor.svelte';

	interface Props {
		controller: CollectionViewController;
		snapshot: CollectionViewState;
		workspaceId: string;
		/** Standalone record page: no close button (the route owns the panel). */
		standalone?: boolean;
	}

	let { controller, snapshot, workspaceId, standalone = false }: Props = $props();

	const panel = $derived(snapshot.panel);
	const row = $derived(
		panel
			? (snapshot.rows.find((candidate) => candidate.recordId === panel.recordId) ?? null)
			: null
	);
	const fields = $derived(snapshot.collection?.fields ?? []);
	const bodyDraft = $derived(panel ? snapshot.bodyDrafts[panel.recordId] : undefined);
	const bodyValue = $derived(bodyDraft ?? panel?.body ?? '');
	const headingId = $derived(`record-panel-title-${panel?.recordId ?? 'none'}`);

	// Ephemeral archive confirmation; the controller refuses without `confirmed === true`.
	let archiveConfirming = $state(false);
	let archiveBusy = $state(false);
	let archiveErrorKey = $state<string | null>(null);
	let archiveFor = $state<string | null>(null);

	$effect(() => {
		const recordId = panel?.recordId ?? null;
		if (recordId !== archiveFor) {
			archiveFor = recordId;
			archiveConfirming = false;
			archiveErrorKey = null;
		}
	});

	async function archive(): Promise<void> {
		if (!panel || archiveBusy) return;
		archiveBusy = true;
		archiveErrorKey = null;
		const outcome = await controller.archiveRecord(panel.recordId, true);
		archiveBusy = false;
		if (outcome.kind === 'failed') archiveErrorKey = outcome.key;
		else if (outcome.kind === 'archived') archiveConfirming = false;
	}

	function onKeydown(event: KeyboardEvent): void {
		if (event.key === 'Escape' && !standalone) {
			event.preventDefault();
			controller.closePanel();
		}
	}
</script>

{#if panel}
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<aside
		class="flex w-full flex-col border-slate-200 bg-white lg:w-96 lg:shrink-0 lg:border-l dark:border-slate-800 dark:bg-slate-950"
		aria-labelledby={headingId}
		data-testid="record-panel"
		data-record-id={panel.recordId}
		onkeydown={onKeydown}
	>
		<header
			class="flex items-center justify-between gap-2 border-b border-slate-200 px-4 py-2 dark:border-slate-800"
		>
			<h2
				id={headingId}
				class="truncate text-base font-semibold text-slate-900 dark:text-slate-100"
			>
				{$t('flow.record.title')}
			</h2>
			{#if !standalone}
				<button
					type="button"
					class="min-h-11 min-w-11 rounded text-slate-600 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-slate-300 dark:hover:bg-slate-800"
					aria-label={$t('flow.record.close')}
					data-testid="record-panel-close"
					onclick={() => controller.closePanel()}>✕</button
				>
			{/if}
		</header>

		{#if panel.status === 'opening'}
			<p class="p-4 text-sm text-slate-500" aria-live="polite">{$t('flow.record.opening')}</p>
		{:else if panel.status === 'error'}
			<p class="p-4 text-sm text-red-600" role="alert">
				{$t(panel.errorKey ?? 'flow.record.openFailed')}
			</p>
		{:else}
			<section
				class="space-y-3 overflow-y-auto px-4 py-3"
				aria-label={$t('flow.record.properties')}
			>
				<h3 class="text-xs font-semibold tracking-wide text-slate-500 uppercase">
					{$t('flow.record.properties')}
				</h3>
				{#if !row}
					<p class="text-sm text-slate-500">{$t('flow.record.notLoaded')}</p>
				{:else}
					{#each fields as field (field.fieldId)}
						{@const draft = draftIn(snapshot, panel.recordId, field.fieldId)}
						{@const status = snapshot.cellStatus[`${panel.recordId}:${field.fieldId}`]}
						{@const editorId = `record-panel-${panel.recordId}-${field.fieldId}`}
						<div class="space-y-1" data-testid="record-panel-field" data-field-id={field.fieldId}>
							<label
								for={editorId}
								class="block text-xs font-medium text-slate-600 dark:text-slate-300"
							>
								{field.label}
							</label>
							{#if isEditableField(field)}
								<CollectionValueEditor
									{field}
									id={editorId}
									value={editorValue(field, row.values, draft)}
									onchange={(value) => controller.setDraft(panel.recordId, field.fieldId, value)}
									oncommit={() => void controller.commitCell(panel.recordId, field.fieldId)}
									oncancel={() => controller.cancelDraft(panel.recordId, field.fieldId)}
								/>
								{#if draft !== null}
									<div class="flex items-center gap-2">
										<button
											type="button"
											class="min-h-11 rounded bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50"
											disabled={status?.state === 'saving'}
											data-testid="record-panel-field-save"
											onclick={() => void controller.commitCell(panel.recordId, field.fieldId)}
											>{$t('flow.record.save')}</button
										>
										<button
											type="button"
											class="min-h-11 rounded px-3 text-sm text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800"
											onclick={() => controller.cancelDraft(panel.recordId, field.fieldId)}
											>{$t('flow.record.discard')}</button
										>
										<span class="text-xs text-amber-700 dark:text-amber-300"
											>{$t('flow.record.unsaved')}</span
										>
									</div>
								{/if}
							{:else}
								<div id={editorId} class="text-sm">
									<CollectionCellValue {field} values={row.values} {workspaceId} />
								</div>
							{/if}
							{#if status && status.state !== 'saving'}
								<p class="text-xs text-red-600 dark:text-red-400" role="alert">
									{status.state === 'conflict'
										? $t('flow.collection.cell.conflict')
										: $t(status.key)}
								</p>
							{/if}
						</div>
					{/each}
				{/if}
			</section>

			<section
				class="space-y-2 border-t border-slate-200 px-4 py-3 dark:border-slate-800"
				aria-label={$t('flow.record.body')}
			>
				{#if panel.bodyWithheld}
					<h3 class="text-xs font-semibold tracking-wide text-slate-500 uppercase">
						{$t('flow.record.body')}
					</h3>
					<p
						class="rounded border border-dashed border-slate-300 p-3 text-sm text-slate-600 dark:border-slate-600 dark:text-slate-300"
						data-testid="record-panel-body-withheld"
					>
						{$t('flow.record.bodyWithheld')}
					</p>
				{:else}
					<label
						for={`record-body-${panel.recordId}`}
						class="block text-xs font-semibold tracking-wide text-slate-500 uppercase"
					>
						{$t('flow.record.body')}
					</label>
					<textarea
						id={`record-body-${panel.recordId}`}
						class="min-h-40 w-full rounded border border-slate-300 bg-white p-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100"
						placeholder={$t('flow.record.bodyPlaceholder')}
						value={bodyValue}
						data-testid="record-panel-body"
						oninput={(event) => controller.setBodyDraft(panel.recordId, event.currentTarget.value)}
					></textarea>
					<div class="flex flex-wrap items-center gap-2">
						<button
							type="button"
							class="min-h-11 rounded bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50"
							disabled={bodyDraft === undefined || panel.bodyStatus?.state === 'saving'}
							data-testid="record-panel-body-save"
							onclick={() => void controller.commitBody(panel.recordId)}
							>{$t('flow.record.saveBody')}</button
						>
						{#if bodyDraft !== undefined}
							<button
								type="button"
								class="min-h-11 rounded px-3 text-sm text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800"
								onclick={() => controller.discardBodyDraft(panel.recordId)}
								>{$t('flow.record.discard')}</button
							>
						{/if}
						<span class="text-xs" aria-live="polite" data-testid="record-panel-body-status">
							{#if panel.bodyStatus?.state === 'saving'}
								{$t('flow.collection.cell.saving')}
							{:else if panel.bodyStatus?.state === 'conflict'}
								<span class="text-red-600">{$t('flow.collection.cell.conflict')}</span>
							{:else if panel.bodyStatus?.state === 'error'}
								<span class="text-red-600">{$t(panel.bodyStatus.key)}</span>
							{:else if bodyDraft !== undefined}
								<span class="text-amber-700 dark:text-amber-300">{$t('flow.record.unsaved')}</span>
							{:else}
								{$t('flow.record.bodySaved')}
							{/if}
						</span>
					</div>
				{/if}
			</section>

			<section
				class="space-y-2 border-t border-slate-200 px-4 py-3 dark:border-slate-800"
				aria-label={$t('flow.record.archive')}
			>
				{#if archiveConfirming}
					<div class="space-y-2" role="group" aria-labelledby={`record-archive-${panel.recordId}`}>
						<p
							id={`record-archive-${panel.recordId}`}
							class="text-sm text-slate-700 dark:text-slate-200"
						>
							{$t('flow.record.archiveConfirmBody')}
						</p>
						<div class="flex flex-wrap items-center gap-2">
							<button
								type="button"
								class="min-h-11 rounded bg-red-700 px-3 text-sm font-medium text-white hover:bg-red-800 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50"
								disabled={archiveBusy}
								data-testid="record-panel-archive-confirm"
								onclick={() => void archive()}>{$t('flow.record.archiveConfirm')}</button
							>
							<button
								type="button"
								class="min-h-11 rounded px-3 text-sm text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800"
								disabled={archiveBusy}
								onclick={() => {
									archiveConfirming = false;
									archiveErrorKey = null;
								}}>{$t('common.cancel')}</button
							>
						</div>
					</div>
				{:else}
					<button
						type="button"
						class="min-h-11 rounded px-3 text-sm text-red-700 hover:bg-red-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-red-300 dark:hover:bg-red-950"
						data-testid="record-panel-archive"
						onclick={() => (archiveConfirming = true)}>{$t('flow.record.archive')}</button
					>
				{/if}
				<p class="text-xs text-red-600 dark:text-red-400" aria-live="polite">
					{#if archiveBusy}
						<span class="text-slate-500">{$t('flow.record.archiving')}</span>
					{:else if archiveErrorKey}
						<span role="alert">{$t(archiveErrorKey)}</span>
					{/if}
				</p>
			</section>
		{/if}
	</aside>
{/if}
