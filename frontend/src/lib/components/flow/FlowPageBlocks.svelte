<script lang="ts">
	// The Page's non-text blocks under the editor: embedded Collections (server-written
	// `embed_type=collection` blocks), Forms Bridge Reference / Embed cards, and the single
	// provisional block of a Collection embed in flight. Also hosts the slash-menu dialogs for those
	// entries. All intent, key and classification logic lives in `$lib/flow/slash-bridge`; this
	// component only keeps dialog state.
	import { getContext, onDestroy, onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import Modal from '$lib/components/Modal.svelte';
	import type { LoroDocType } from 'loro-prosemirror';
	import type { FlowReferenceItem } from '$lib/api/flow';
	import type { FlowObjectRepository } from '$lib/flow/object-repository';
	import { FLOW_REPOSITORY_CONTEXT } from '$lib/flow/context-keys';
	import { FlowCommandService } from '$lib/flow/command-service';
	import {
		BridgeReferenceSession,
		CollectionEmbedSession,
		COLLECTION_EMBED_TITLE_MAX_CHARS,
		FORMS_PICKER_READS,
		FormsTargetPicker,
		PageBlockTree,
		buildPageCards,
		validateEmbedTitle,
		type CollectionEmbedSnapshot,
		type FormsPickerSnapshot,
		type PageBlock,
		type PageDocument,
		type ReferenceMode,
		type SlashBridgeAction,
		type SlashFailure
	} from '$lib/flow/slash-bridge';
	import EmbeddedCollection from './collection/EmbeddedCollection.svelte';
	import BridgeReferenceCard from './bridge/BridgeReferenceCard.svelte';

	interface Props {
		doc: LoroDocType;
		workspaceId: string;
		objectId: string;
	}

	let { doc, workspaceId, objectId }: Props = $props();

	const repository = getContext<FlowObjectRepository>(FLOW_REPOSITORY_CONTEXT);
	const commands = new FlowCommandService();
	// svelte-ignore state_referenced_locally
	const tree = new PageBlockTree(doc as unknown as PageDocument);
	// svelte-ignore state_referenced_locally
	const embed = new CollectionEmbedSession(objectId, commands, (snapshot) => {
		embedSnap = snapshot;
	});
	// svelte-ignore state_referenced_locally
	const references = new BridgeReferenceSession(objectId, commands, repository);

	let blocks = $state<PageBlock[]>([]);
	let referenceItems = $state<FlowReferenceItem[]>([]);
	let referencesFailure = $state<SlashFailure | null>(null);
	let embedSnap = $state<CollectionEmbedSnapshot>(embed.snapshot());
	let announcement = $state('');

	const cards = $derived(buildPageCards(blocks, referenceItems, embedSnap.provisional));

	// Collection dialog.
	let collectionOpen = $state(false);
	let collectionTitle = $state('');
	let collectionInvalid = $state<string | null>(null);

	// Forms Reference / Embed dialog.
	let formsOpen = $state(false);
	let formsMode = $state<ReferenceMode>('reference');
	let picker = $state<FormsTargetPicker | null>(null);
	let pickerSnap = $state<FormsPickerSnapshot | null>(null);
	let formsSubmitting = $state(false);
	let formsFailure = $state<SlashFailure | null>(null);

	// Remove confirmation.
	let removingId = $state<string | null>(null);
	let removeBusy = $state(false);
	let removeFailure = $state<SlashFailure | null>(null);

	let unsubscribeTree: (() => void) | null = null;

	onMount(() => {
		unsubscribeTree = tree.subscribe((next) => {
			blocks = next;
			embed.settle(new Set(next.map((block) => block.blockId)));
		});
		void reloadReferences();
	});

	onDestroy(() => unsubscribeTree?.());

	function announce(key: string, values?: Record<string, unknown>): void {
		announcement = get(t)(key, { values });
	}

	async function reloadReferences(): Promise<void> {
		const outcome = await references.list();
		if (outcome.kind === 'ok') {
			referenceItems = outcome.items;
			referencesFailure = null;
		} else {
			referencesFailure = outcome.failure;
		}
	}

	/** Entry point for the slash menu. */
	export function open(action: SlashBridgeAction): void {
		if (action === 'collection') {
			collectionTitle = get(t)('flow.canvas.slashMenu.collectionDefaultTitle');
			collectionInvalid = null;
			collectionOpen = true;
		} else if (action === 'reference' || action === 'embed') {
			formsMode = action;
			formsFailure = null;
			formsSubmitting = false;
			const next = new FormsTargetPicker(workspaceId, FORMS_PICKER_READS, (snapshot) => {
				pickerSnap = snapshot;
			});
			picker = next;
			pickerSnap = next.snapshot();
			formsOpen = true;
			void next.load();
		} else {
			void goto(
				resolve('/(app)/workspace/[workspaceId]/flow/[objectId]/convert', {
					workspaceId,
					objectId
				})
			);
		}
	}

	async function submitCollection(event?: SubmitEvent): Promise<void> {
		event?.preventDefault();
		const labels = {
			fieldLabel: get(t)('flow.canvas.slashMenu.collectionFieldLabel'),
			viewName: get(t)('flow.nav.createCollection.viewName.table')
		};
		if (embedSnap.busy) return;
		const validation = validateEmbedTitle(collectionTitle);
		if (validation !== 'ok') {
			collectionInvalid =
				validation === 'title_required'
					? 'flow.canvas.slashMenu.collectionTitleRequired'
					: 'flow.canvas.slashMenu.collectionTitleTooLong';
			return;
		}
		collectionInvalid = null;
		// The dialog closes as soon as the request is under way: the provisional block is the
		// progress indicator.
		collectionOpen = false;
		announce('flow.canvas.slashMenu.collectionPending');
		await afterEmbed(await embed.start(collectionTitle, labels));
	}

	async function retryCollection(): Promise<void> {
		announce('flow.canvas.slashMenu.collectionPending');
		await afterEmbed(await embed.retry());
	}

	async function afterEmbed(outcome: Awaited<ReturnType<CollectionEmbedSession['start']>>) {
		if (outcome.kind === 'created') {
			announce('flow.canvas.slashMenu.collectionCreated');
			if (outcome.collectionId) repository.notifyObjectCreated(outcome.collectionId, objectId);
			await repository.resyncAccepted(objectId);
		} else if (outcome.kind === 'failed') {
			announce(outcome.failure.key);
		}
	}

	async function submitForms(event?: SubmitEvent): Promise<void> {
		event?.preventDefault();
		const target = pickerSnap?.target;
		if (!target || formsSubmitting) return;
		formsSubmitting = true;
		formsFailure = null;
		const outcome = await references.create(target, formsMode);
		formsSubmitting = false;
		if (outcome.kind === 'created') {
			tree.addReferencePlaceholder(outcome.receipt.reference_id, outcome.mode);
			formsOpen = false;
			announce('flow.bridge.card.created');
			await reloadReferences();
		} else if (outcome.kind === 'failed') {
			formsFailure = outcome.failure;
		}
	}

	function askRemove(referenceId: string): void {
		removingId = referenceId;
		removeFailure = null;
	}

	async function confirmRemove(): Promise<void> {
		const referenceId = removingId;
		if (!referenceId || removeBusy) return;
		removeBusy = true;
		const outcome = await references.remove(referenceId, true);
		removeBusy = false;
		if (outcome.kind === 'removed') {
			tree.removeReferencePlaceholders(referenceId);
			removingId = null;
			announce('flow.bridge.card.removed');
			await reloadReferences();
		} else if (outcome.kind === 'failed') {
			removeFailure = outcome.failure;
		}
	}

	const selectClass =
		'mt-1 block min-h-11 w-full rounded-md border border-slate-300 bg-white px-3 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-900 dark:text-slate-100';
	const buttonClass =
		'inline-flex min-h-11 items-center rounded-md px-4 text-sm font-medium focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50';
</script>

<div class="sr-only" aria-live="polite" data-testid="flow-page-blocks-live">{announcement}</div>

{#if cards.length > 0 || embedSnap.failure || referencesFailure}
	<div class="mx-auto max-w-3xl" data-testid="flow-page-blocks">
		{#each cards as card (card.kind === 'collection' ? card.blockId : card.kind === 'provisional' ? `provisional-${card.provisional.localId}` : card.key)}
			{#if card.kind === 'collection'}
				<EmbeddedCollection {workspaceId} collectionId={card.collectionId} blockId={card.blockId} />
			{:else if card.kind === 'provisional'}
				<section
					class="not-prose my-4 rounded-lg border border-dashed border-slate-300 bg-slate-50 p-4 dark:border-slate-600 dark:bg-slate-900"
					data-testid="flow-embed-provisional"
					data-status={card.provisional.status}
					aria-busy="true"
				>
					<p class="text-sm font-semibold text-slate-900 dark:text-slate-100">
						{card.provisional.title}
					</p>
					<p class="text-xs text-slate-500 dark:text-slate-400">
						{$t('flow.canvas.slashMenu.collectionPending')}
					</p>
				</section>
			{:else if card.kind === 'reference'}
				<BridgeReferenceCard
					{workspaceId}
					reference={card.reference}
					mode={card.mode}
					onremove={askRemove}
				/>
			{:else}
				<article
					class="not-prose my-3 flex flex-wrap items-center gap-2 rounded-lg border border-slate-200 p-4 dark:border-slate-700"
					data-testid="flow-bridge-card-unavailable"
					data-reference-id={card.referenceId}
				>
					<p class="flex-1 text-sm text-slate-500 dark:text-slate-400">
						{$t('flow.bridge.card.unavailable')}
					</p>
					<button
						type="button"
						class="{buttonClass} text-red-700 hover:bg-red-50 dark:text-red-300 dark:hover:bg-red-950"
						onclick={() => askRemove(card.referenceId)}
					>
						{$t('flow.bridge.card.remove')}
					</button>
				</article>
			{/if}
		{/each}

		{#if embedSnap.failure}
			<div
				class="not-prose my-3 flex flex-wrap items-center gap-3 rounded-lg border border-red-200 bg-red-50 p-3 text-sm text-red-900 dark:border-red-900 dark:bg-red-950 dark:text-red-100"
				role="alert"
				data-testid="flow-embed-failed"
				data-error-code={embedSnap.failure.code}
			>
				<span class="flex-1">
					{$t('flow.canvas.slashMenu.collectionFailedTitle', {
						values: { title: embedSnap.failedTitle ?? '' }
					})}
					{$t(embedSnap.failure.key)}
				</span>
				{#if embedSnap.failure.retryable}
					<button
						type="button"
						class="{buttonClass} bg-red-700 text-white hover:bg-red-800"
						disabled={embedSnap.busy}
						data-testid="flow-embed-retry"
						onclick={() => void retryCollection()}
					>
						{$t('flow.canvas.slashMenu.retry')}
					</button>
				{/if}
				<button
					type="button"
					class="{buttonClass} underline"
					disabled={embedSnap.busy}
					onclick={() => embed.dismiss()}
				>
					{$t('flow.recovery.dismiss')}
				</button>
			</div>
		{/if}

		{#if referencesFailure}
			<p class="not-prose my-3 text-sm text-red-600" role="alert">
				{$t(referencesFailure.key)}
			</p>
		{/if}
	</div>
{/if}

<Modal
	open={collectionOpen}
	title={$t('flow.canvas.slashMenu.collectionDialogTitle')}
	onclose={() => (collectionOpen = false)}
>
	<form class="space-y-4" onsubmit={submitCollection} data-testid="flow-slash-collection-dialog">
		<p class="text-sm text-slate-600 dark:text-slate-300">
			{$t('flow.canvas.slashMenu.collectionNote')}
		</p>
		<label class="block text-sm font-medium text-slate-700 dark:text-slate-200">
			{$t('flow.canvas.slashMenu.collectionTitleLabel')}
			<input
				class={selectClass}
				bind:value={collectionTitle}
				maxlength={COLLECTION_EMBED_TITLE_MAX_CHARS}
				data-testid="flow-slash-collection-title"
			/>
		</label>
		{#if collectionInvalid}
			<p class="text-sm text-red-600" role="alert">{$t(collectionInvalid)}</p>
		{/if}
		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="{buttonClass} text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-700"
				onclick={() => (collectionOpen = false)}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="submit"
				class="{buttonClass} bg-blue-600 text-white hover:bg-blue-700"
				disabled={embedSnap.busy}
				data-testid="flow-slash-collection-submit"
			>
				{$t('flow.canvas.slashMenu.collectionSubmit')}
			</button>
		</div>
	</form>
</Modal>

<Modal
	open={formsOpen}
	title={$t(
		formsMode === 'embed'
			? 'flow.canvas.slashMenu.embedDialogTitle'
			: 'flow.canvas.slashMenu.referenceDialogTitle'
	)}
	maxWidthClass="max-w-lg"
	onclose={() => (formsOpen = false)}
>
	<form
		class="space-y-4"
		onsubmit={submitForms}
		data-testid="flow-slash-forms-dialog"
		data-mode={formsMode}
	>
		<p class="text-sm text-slate-600 dark:text-slate-300" data-testid="flow-slash-forms-note">
			{$t(
				formsMode === 'embed'
					? 'flow.canvas.slashMenu.embedNote'
					: 'flow.canvas.slashMenu.referenceNote'
			)}
		</p>
		{#if pickerSnap}
			<label class="block text-sm font-medium text-slate-700 dark:text-slate-200">
				{$t('flow.bridge.card.picker.project')}
				<select
					class={selectClass}
					data-testid="flow-slash-forms-project"
					value={pickerSnap.projectId ?? ''}
					disabled={pickerSnap.projectsState !== 'ready'}
					onchange={(event) => void picker?.selectProject(event.currentTarget.value || null)}
				>
					<option value="">{$t('flow.bridge.card.picker.chooseProject')}</option>
					{#each pickerSnap.projects as project (project.id)}
						<option value={project.id}>{project.label}</option>
					{/each}
				</select>
			</label>
			{#if pickerSnap.projectsState === 'ready' && pickerSnap.projects.length === 0}
				<p class="text-sm text-slate-500">{$t('flow.bridge.card.picker.noProjects')}</p>
			{/if}
			{#if pickerSnap.projectId}
				<label class="block text-sm font-medium text-slate-700 dark:text-slate-200">
					{$t('flow.bridge.card.picker.form')}
					<select
						class={selectClass}
						data-testid="flow-slash-forms-form"
						value={pickerSnap.formId ?? ''}
						disabled={pickerSnap.formsState !== 'ready'}
						onchange={(event) => void picker?.selectForm(event.currentTarget.value || null)}
					>
						<option value="">{$t('flow.bridge.card.picker.chooseForm')}</option>
						{#each pickerSnap.forms as form (form.id)}
							<option value={form.id}>{form.name}</option>
						{/each}
					</select>
				</label>
				{#if pickerSnap.formsState === 'failed'}
					<p class="text-sm text-red-600" role="alert">
						{$t('flow.bridge.card.picker.loadFailed')}
					</p>
				{/if}
			{/if}
			{#if pickerSnap.formId}
				<label class="block text-sm font-medium text-slate-700 dark:text-slate-200">
					{$t('flow.bridge.card.picker.record')}
					<select
						class={selectClass}
						data-testid="flow-slash-forms-record"
						value={pickerSnap.recordId ?? ''}
						disabled={pickerSnap.recordsState !== 'ready'}
						onchange={(event) => picker?.selectRecord(event.currentTarget.value || null)}
					>
						<option value="">{$t('flow.bridge.card.picker.wholeForm')}</option>
						{#each pickerSnap.records as record (record.id)}
							<option value={record.id}>{record.title || record.id}</option>
						{/each}
					</select>
				</label>
				{#if pickerSnap.recordsState === 'failed'}
					<p class="text-sm text-red-600" role="alert">
						{$t('flow.bridge.card.picker.loadFailed')}
					</p>
				{/if}
			{/if}
		{/if}
		{#if formsFailure}
			<p class="text-sm text-red-600" role="alert" data-error-code={formsFailure.code}>
				{$t(formsFailure.key)}
			</p>
		{/if}
		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="{buttonClass} text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-700"
				onclick={() => (formsOpen = false)}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="submit"
				class="{buttonClass} bg-blue-600 text-white hover:bg-blue-700"
				disabled={!pickerSnap?.target ||
					formsSubmitting ||
					(formsFailure !== null && !formsFailure.retryable)}
				data-testid="flow-slash-forms-submit"
			>
				{$t(
					formsFailure?.retryable
						? 'flow.canvas.slashMenu.retry'
						: formsMode === 'embed'
							? 'flow.canvas.slashMenu.embedSubmit'
							: 'flow.canvas.slashMenu.referenceSubmit'
				)}
			</button>
		</div>
	</form>
</Modal>

<Modal
	open={removingId !== null}
	title={$t('flow.bridge.card.removeTitle')}
	onclose={() => (removingId = null)}
>
	<div class="space-y-4" data-testid="flow-bridge-remove-dialog">
		<p class="text-sm text-slate-700 dark:text-slate-200">{$t('flow.bridge.card.removeBody')}</p>
		{#if removeFailure}
			<p class="text-sm text-red-600" role="alert">{$t(removeFailure.key)}</p>
		{/if}
		<div class="flex justify-end gap-2">
			<button
				type="button"
				class="{buttonClass} text-slate-700 hover:bg-slate-100 dark:text-slate-200 dark:hover:bg-slate-700"
				onclick={() => (removingId = null)}
			>
				{$t('common.cancel')}
			</button>
			<button
				type="button"
				class="{buttonClass} bg-red-700 text-white hover:bg-red-800"
				disabled={removeBusy || (removeFailure !== null && !removeFailure.retryable)}
				data-testid="flow-bridge-remove-confirm"
				onclick={() => void confirmRemove()}
			>
				{$t('flow.bridge.card.removeConfirm')}
			</button>
		</div>
	</div>
</Modal>
