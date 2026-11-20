<script lang="ts">
	import { onMount, onDestroy, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import Modal from '../Modal.svelte';
	import { flowApi, type FlowObjectView } from '#lib/api/flow.js';
	import type {
		TransferPageRuntime,
		TransferRuntimePreview
	} from '#lib/flow/block-transfer/runtime.js';
	import type { TransferOperationState } from '#lib/flow/block-transfer/controller.js';
	let {
		open = $bindable(false),
		runtime,
		rootCid,
		pickerScopeId,
		initialKind = 'move'
	}: {
		open?: boolean;
		runtime: TransferPageRuntime;
		rootCid: string;
		pickerScopeId: string;
		initialKind?: 'move' | 'copy';
	} = $props();
	let snapshot = $state<TransferOperationState>(untrack(() => runtime.operation.current())),
		preview = $state<TransferRuntimePreview | null>(null);
	let kind = $state<'move' | 'copy'>(
			untrack(() =>
				rootCid ? initialKind : (runtime.operation.current().review?.kind ?? initialKind)
			)
		),
		selection = $state<'block' | 'current_subtree'>('block'),
		query = $state(''),
		pages = $state<FlowObjectView[]>([]),
		nextCursor = $state<string | undefined>();
	let targetId = $state(''),
		parentCid = $state<string | null>(null),
		index = $state(0),
		consent = $state(false),
		searching = $state(false),
		loadingTarget = $state(false),
		uiError = $state(''),
		formChanged = $state(true),
		preparing = $state(false);
	let searchInput: HTMLInputElement | undefined = $state();
	let dead = false,
		generation = 0;
	const blocked = $derived(
		preparing || ['preparing', 'committing', 'unknown', 'undoing'].includes(snapshot.phase)
	);
	onMount(() => {
		const stop = runtime.operation.subscribe((value) => {
			if (value.error === 'review_changed') {
				consent = false;
				formChanged = true;
			}
			snapshot = value;
			if (value.detailsUnavailable) {
				preview = null;
				pages = [];
			}
		});
		if (rootCid) {
			searchInput?.focus({ preventScroll: true });
			void search();
		}
		return stop;
	});
	onDestroy(() => {
		dead = true;
		generation++;
		if (!['committing', 'unknown', 'undoing'].includes(runtime.operation.current().phase))
			void runtime.releasePins();
	});
	function changed() {
		formChanged = true;
		consent = false;
		uiError = '';
	}
	async function search(append = false) {
		const request = ++generation;
		searching = true;
		const result = await flowApi.listObjects(pickerScopeId, {
			object_type: 'page',
			q: query,
			limit: 50,
			...(append && nextCursor ? { cursor: nextCursor } : {})
		});
		if (dead || request !== generation || !runtime.allowed()) return;
		searching = false;
		if (result.code !== 0 || !result.data) {
			pages = [];
			uiError = 'target_unavailable';
			return;
		}
		const results = result.data.items.filter(
			(page) =>
				page.id !== runtime.sourceId &&
				page.scope_id === pickerScopeId &&
				page.lifecycle_status === 'active' &&
				page.permitted_actions?.includes(kind === 'move' ? 'manage' : 'edit') &&
				(!page.editor_state || page.editor_state === 'ready')
		);
		pages = append ? [...pages, ...results] : results;
		nextCursor = result.data.next_cursor;
	}
	async function target(page: FlowObjectView) {
		if (blocked || loadingTarget) return;
		preview = null;
		targetId = page.id;
		changed();
		loadingTarget = true;
		try {
			preview = await runtime.loadTarget(page.id);
			parentCid = null;
			index = preview.targetBlocks.filter((block) => block.depth === 0).length;
		} catch (error) {
			uiError = error instanceof Error ? error.message : 'target_unavailable';
			preview = null;
		} finally {
			loadingTarget = false;
		}
	}
	function children(parent: string | null): number {
		if (!preview) return 0;
		if (parent === null) return preview.targetBlocks.filter((block) => block.depth === 0).length;
		const blocks = preview.targetBlocks,
			i = blocks.findIndex((block) => block.cid === parent);
		if (i < 0) return 0;
		let count = 0;
		for (let n = i + 1; n < blocks.length && blocks[n].depth > blocks[i].depth; n++)
			if (blocks[n].depth === blocks[i].depth + 1) count++;
		return count;
	}
	async function prepare() {
		if (!targetId || blocked) return;
		preparing = true;
		uiError = '';
		try {
			const prepared = await runtime.prepare({
				kind,
				rootCid,
				selection,
				targetId,
				parentCid,
				index
			});
			preview = runtime.preview;
			if (prepared) {
				formChanged = false;
				consent = false;
			}
		} catch (error) {
			uiError = error instanceof Error ? error.message : 'review_failed';
		} finally {
			preparing = false;
		}
	}
	function pageHref(objectId: string) {
		return page.params.workspaceId
			? resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
					workspaceId: page.params.workspaceId,
					objectId
				})
			: resolve('/(app)/flow/[objectId]', { objectId });
	}

	const error = $derived(
		uiError === 'open_and_sync' || uiError === 'unsaved_content'
			? $t('flow.transfer.openAndSync')
			: uiError === 'explicit_subtree_required'
				? $t('flow.transfer.explicitSubtree')
				: uiError === 'source_permission' || uiError === 'target_permission'
					? $t('flow.transfer.permissionRequired')
					: uiError
						? $t('flow.transfer.reviewFailed')
						: snapshot.error === 'review_changed'
							? $t('flow.transfer.reviewChanged')
							: snapshot.error === 'dirty_pages'
								? $t('flow.transfer.openAndSync')
								: snapshot.error === 'confirmation_required'
									? $t('flow.transfer.confirmBoundary')
									: snapshot.error === 'undo_conflict'
										? $t('flow.transfer.undoConflict')
										: snapshot.error
											? $t('flow.transfer.actionFailed')
											: ''
	);
</script>

<Modal
	bind:open
	taskScoped
	title={$t(kind === 'move' ? 'flow.transfer.moveToPage' : 'flow.transfer.copyToPage')}
	maxWidthClass="max-w-2xl"
>
	<div class="space-y-4" data-testid="flow-block-transfer" data-phase={snapshot.phase}>
		{#if rootCid && !['committing', 'unknown', 'undoing', 'applied', 'undone'].includes(snapshot.phase)}
			<fieldset class="space-y-2 text-sm" disabled={blocked}>
				<legend class="font-medium">{$t('flow.transfer.selection')}</legend><label class="block"
					><input
						type="radio"
						name="transfer-selection"
						checked={selection === 'block'}
						onchange={() => {
							selection = 'block';
							changed();
						}}
					/>
					{$t('flow.transfer.onlyBlock')}</label
				><label class="block"
					><input
						type="radio"
						name="transfer-selection"
						checked={selection === 'current_subtree'}
						onchange={() => {
							selection = 'current_subtree';
							changed();
						}}
					/>
					{$t('flow.transfer.subtree')}</label
				>
			</fieldset>
			<form
				class="flex gap-2"
				onsubmit={(event) => {
					event.preventDefault();
					void search();
				}}
			>
				<label class="min-w-0 flex-1 text-sm"
					>{$t('flow.transfer.targetPage')}<input
						class="mt-1 block w-full rounded border p-2"
						type="search"
						bind:this={searchInput}
						bind:value={query}
						disabled={blocked}
					/></label
				><button
					type="submit"
					class="self-end rounded border px-3 py-2 text-sm"
					disabled={blocked || searching}>{$t('common.search')}</button
				>
			</form>
			<div
				class="max-h-40 space-y-1 overflow-y-auto rounded border p-2"
				aria-label={$t('flow.transfer.authorizedPages')}
			>
				{#each pages as page (page.id)}<button
						type="button"
						class="block w-full rounded px-2 py-2 text-left text-sm hover:bg-surface"
						aria-pressed={targetId === page.id}
						disabled={blocked || loadingTarget}
						onclick={() => target(page)}>{page.title || $t('flow.nav.untitled')}</button
					>{/each}{#if searching}<p role="status" class="text-sm text-fg-muted">
						{$t('common.loading')}
					</p>{:else if !pages.length}<p class="text-sm text-fg-muted">
						{$t('flow.transfer.noTargets')}
					</p>{/if}
			</div>
			{#if nextCursor}<button
					type="button"
					class="text-sm underline"
					disabled={searching || blocked}
					onclick={() => search(true)}>{$t('common.loadMore')}</button
				>{/if}
			{#if preview && !snapshot.detailsUnavailable}<section
					class="space-y-2 rounded border p-3"
					data-testid="flow-transfer-placement"
				>
					<p class="text-sm font-medium">
						{preview.source.title || $t('flow.nav.untitled')} → {preview.target.title ||
							$t('flow.nav.untitled')}
					</p>
					<label class="block text-sm"
						>{$t('flow.transfer.targetParent')}<select
							class="mt-1 block w-full rounded border p-2"
							value={parentCid ?? ''}
							disabled={blocked}
							onchange={(event) => {
								parentCid = event.currentTarget.value || null;
								index = children(parentCid);
								changed();
							}}
							><option value="">{$t('flow.transfer.rootLevel')}</option
							>{#each preview.targetBlocks as block (block.cid)}<option value={block.cid}
									>{'　'.repeat(block.depth)}{block.text || $t('flow.transfer.block')}</option
								>{/each}</select
						></label
					>
					<label class="block text-sm"
						>{$t('flow.transfer.targetIndex')}<select
							class="mt-1 block rounded border p-2"
							value={index}
							disabled={blocked}
							onchange={(event) => {
								index = Number(event.currentTarget.value);
								changed();
							}}
							>{#each Array.from({ length: children(parentCid) + 1 }, (_, position) => position) as position (position)}<option
									value={position}
									>{position === children(parentCid)
										? $t('flow.transfer.end')
										: position + 1}</option
								>{/each}</select
						></label
					>
				</section>{/if}
			<button
				type="button"
				class="rounded border px-3 py-2 text-sm disabled:text-fg-muted disabled:hover:bg-transparent"
				data-testid="flow-transfer-review"
				disabled={!targetId || blocked || loadingTarget || snapshot.phase === 'preparing'}
				onclick={prepare}
				>{preparing ? $t('flow.transfer.preparing') : $t('flow.transfer.review')}</button
			>
			{#if snapshot.review && preview && !formChanged && !snapshot.detailsUnavailable}<section
					class="space-y-3 rounded border p-3"
					data-testid="flow-transfer-fixed-review"
				>
					<p class="text-sm font-medium">
						{$t('flow.transfer.fixedCount', { values: { count: snapshot.review.memberCount } })}
					</p>
					<ol class="max-h-44 space-y-2 overflow-y-auto text-sm">
						{#each preview.selected as block (block.cid)}<li
								style={`margin-inline-start:${block.depth * 16}px`}
							>
								{block.text || $t('flow.transfer.block')}
							</li>{/each}
					</ol>
					<p class="text-xs text-fg-muted">
						{$t('flow.transfer.assetsAndShares', {
							values: {
								assets: snapshot.review.assetCount,
								shares: snapshot.review.affectedShareCount
							}
						})}
					</p>
					{#if kind === 'copy' || snapshot.review.affectedShareCount > 0}
						<p class="text-sm text-fg" data-testid="flow-transfer-share-impact">
							{$t(
								kind === 'copy' ? 'flow.transfer.copyShareImpact' : 'flow.transfer.moveShareImpact'
							)}
						</p>
					{/if}
					<p class="text-sm text-warning">
						{kind === 'copy'
							? $t('flow.transfer.copyUndoBoundary')
							: $t('flow.transfer.moveUndoBoundary')}
					</p>
					<label class="flex gap-2 text-sm"
						><input type="checkbox" bind:checked={consent} disabled={blocked} />{$t(
							'flow.transfer.acceptBoundary'
						)}</label
					>
					<button
						type="button"
						class="rounded bg-primary rf-filled px-3 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted"
						data-testid="flow-transfer-commit"
						disabled={blocked || !consent || snapshot.phase !== 'prepared'}
						onclick={() => runtime.operation.commit(consent)}
						>{kind === 'move'
							? $t('flow.transfer.confirmMove')
							: $t('flow.transfer.confirmCopy')}</button
					>
				</section>{/if}
		{/if}
		{#if blocked}<section
				role="status"
				class="space-y-2 rounded border border-warning p-3 text-sm"
				data-testid="flow-transfer-pending"
			>
				<p>{$t('flow.transfer.pending')}</p>
				{#if snapshot.phase === 'unknown'}<button
						type="button"
						class="me-3 underline"
						onclick={() => runtime.operation.resolve()}>{$t('flow.transfer.checkReceipt')}</button
					><button type="button" class="underline" onclick={() => runtime.operation.retryUnknown()}
						>{$t('flow.transfer.retrySame')}</button
					>{/if}
			</section>{/if}
		{#if snapshot.phase === 'applied' || snapshot.phase === 'undone'}<section
				role="status"
				class="space-y-3 rounded border p-3 text-sm"
				data-testid="flow-transfer-receipt"
			>
				<p>
					{snapshot.phase === 'undone' ? $t('flow.transfer.undone') : $t('flow.transfer.applied')}
				</p>
				{#if !snapshot.detailsUnavailable && snapshot.review?.kind === 'move' && snapshot.review.affectedShareCount > 0}
					<p>{$t('flow.transfer.shareReviewAfterTransfer')}</p>
				{/if}
				{#if snapshot.detailsUnavailable}<p>
						{$t('flow.transfer.detailsUnavailable')}
					</p>{:else if snapshot.receipt}<div class="flex flex-wrap gap-3">
						<a
							class="underline"
							href={pageHref(
								snapshot.phase === 'undone' ? runtime.sourceId : snapshot.receipt.targetPageId
							)}
							>{snapshot.phase === 'undone'
								? $t('flow.transfer.openSource')
								: $t('flow.transfer.openTarget')}</a
						>
						{#if snapshot.phase === 'applied' && snapshot.review?.kind === 'move' && snapshot.review.affectedShareCount > 0}
							<a class="underline" href={pageHref(runtime.sourceId)}
								>{$t('flow.transfer.openSource')}</a
							>
						{/if}
						{#if snapshot.phase === 'applied'}<button
								type="button"
								class="underline"
								data-testid="flow-transfer-undo"
								onclick={() => runtime.operation.undo()}
								>{kind === 'move'
									? $t('flow.transfer.undoMove')
									: $t('flow.transfer.undoCopy')}</button
							>{/if}
					</div>{/if}
			</section>{/if}
		{#if snapshot.phase === 'prepared' || snapshot.phase === 'unknown'}<button
				type="button"
				class="text-sm underline"
				onclick={() => runtime.operation.cancel()}>{$t('flow.transfer.cancelPrepared')}</button
			>{/if}
		{#if error}<p role="alert" class="text-sm text-danger">{error}</p>{/if}
	</div>
</Modal>
