<script lang="ts">
	import { t } from 'svelte-i18n';
	import {
		BODY_UPLOAD_LABELS,
		BODY_ERROR_LABELS,
		BODY_MODAL_LABELS,
		type BodyErrorReason
	} from '#lib/flow/body/labels.js';
	import Modal from '#lib/components/Modal.svelte';
	import { flowApi, type FlowObjectView } from '#lib/api/flow.js';
	import type { PageBodyJobs, BodyDraft, BodyJobsSnapshot } from '#lib/flow/body/jobs.js';
	import type { BodyInsertionBookmark } from '#lib/flow/body/assets.js';
	let {
		jobs,
		scopeId,
		readOnly = false,
		bookmark
	}: {
		jobs: PageBodyJobs;
		scopeId: string;
		readOnly?: boolean;
		bookmark: () => BodyInsertionBookmark | null;
	} = $props();
	let jobsState = $state<BodyJobsSnapshot>({ draft: null, uploads: [], children: [], pending: 0 });
	$effect(() => jobs.subscribe((value) => (jobsState = value)));
	let error = $state<BodyErrorReason | ''>(''),
		query = $state(''),
		results = $state<FlowObjectView[]>([]),
		searching = $state(false),
		submitting = $state(false);
	const draft = $derived(jobsState.draft);
	export function open(
		mode: BodyDraft['mode'],
		target: BodyInsertionBookmark,
		kind: 'image' | 'attachment' = 'image',
		attrs: Record<string, unknown> = {}
	): void {
		if (readOnly) return;
		error = '';
		results = [];
		query = '';
		jobs.setDraft({
			mode,
			kind,
			bookmark: { ...target },
			alt: String(attrs.alt ?? ''),
			caption: String(attrs.caption ?? ''),
			title: '',
			targetId: '',
			file: null,
			dirty: false
		});
	}
	function update(patch: Partial<BodyDraft>) {
		if (draft) jobs.setDraft({ ...draft, ...patch, dirty: true });
	}
	function close() {
		jobs.setDraft(null);
		error = '';
	}
	function chooseCurrent() {
		const target = bookmark();
		if (target && draft) update({ bookmark: target });
		else error = 'position';
	}
	async function search() {
		searching = true;
		const response = await flowApi.listObjects(scopeId, {
			object_type: 'page',
			q: query,
			limit: 50
		});
		results =
			response.code === 0
				? (response.data?.items.filter((item) => item.lifecycle_status === 'active') ?? [])
				: [];
		if (response.code !== 0) error = 'search';
		searching = false;
	}
	function targetId(value: string): string | null {
		const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
		if (uuid.test(value.trim())) return value.trim();
		try {
			const url = new URL(value, window.location.origin);
			if (url.origin !== window.location.origin) return null;
			const match = /\/flow\/([0-9a-f-]+)$/.exec(url.pathname);
			return match && uuid.test(match[1]) ? match[1] : null;
		} catch {
			return null;
		}
	}
	async function submit() {
		if (!draft || readOnly || submitting) return;
		error = '';
		const intent = draft;
		submitting = true;
		try {
			if (intent.mode === 'upload' || intent.mode === 'replace') {
				if (!intent.file) {
					error = 'fileRequired';
					return;
				}
				jobs.upload(intent.kind, intent.file, intent.bookmark, {
					alt: intent.alt,
					caption: intent.caption,
					replace: intent.mode === 'replace'
				});
				close();
			} else if (intent.mode === 'child') {
				if (!intent.title.trim()) {
					error = 'titleRequired';
					return;
				}
				jobs.createChild(intent.title.trim(), intent.bookmark);
				close();
			} else if (intent.mode === 'details') {
				if (
					await jobs.details(intent.kind, intent.bookmark, {
						alt: intent.alt,
						caption: intent.caption
					})
				)
					close();
				else error = 'position';
			} else {
				const id = targetId(intent.targetId);
				if (!id) {
					error = 'pageRequired';
					return;
				}
				const checked = await flowApi.getObject(id);
				if (checked.code !== 0 || checked.data?.object_type !== 'page') {
					error = 'pageUnavailable';
					return;
				}
				if (await jobs.linkPage(id, intent.bookmark)) close();
				else error = 'position';
			}
		} finally {
			submitting = false;
		}
	}
	const bytes = (value: number) =>
		value < 1024
			? `${value} B`
			: value < 1024 * 1024
				? `${(value / 1024).toFixed(1)} KiB`
				: `${(value / 1024 / 1024).toFixed(1)} MiB`;
</script>

{#if jobsState.uploads.length || jobsState.children.length}<section
		class="mx-auto mb-4 max-w-3xl space-y-2"
		aria-label={$t('flow.body.pending')}
		data-testid="flow-body-jobs"
	>
		{#each jobsState.uploads as upload (upload.job.key)}{@const job = upload.job}
			<div
				class="rounded-card border border-line p-3 text-sm"
				data-testid="flow-body-upload-job"
				data-state={upload.state.phase}
			>
				<p class="truncate">{job.file.name}</p>
				<p role="status" class="text-xs text-fg-muted">
					{$t(BODY_UPLOAD_LABELS[upload.state.phase])}
				</p>
				{#if upload.state.phase === 'uploading'}<progress
						class="mt-2 w-full"
						aria-label={$t('flow.body.uploadProgress')}
						max={upload.state.total ?? 1}
						value={upload.state.total ? upload.state.loaded : undefined}
					></progress><button
						type="button"
						class="mt-2 text-xs underline"
						onclick={() => job.cancel()}>{$t('common.cancel')}</button
					>{/if}
				{#if upload.state.error}<p role="alert" class="mt-1 text-xs text-danger">
						{$t(BODY_ERROR_LABELS[upload.state.error], {
							values: { limit: bytes(upload.state.maxBytes ?? 0) }
						})}
					</p>{/if}
				{#if ['failed', 'canceled', 'staged'].includes(upload.state.phase)}<div
						class="mt-2 flex gap-3 text-xs"
					>
						<button type="button" disabled={readOnly} onclick={() => jobs.retryUpload(job)}
							>{$t('flow.body.retry')}</button
						>{#if upload.state.error === 'position'}<button
								type="button"
								disabled={readOnly}
								onclick={() => {
									const target = bookmark();
									if (target) jobs.repositionUpload(job, target);
								}}>{$t('flow.body.choosePosition')}</button
							>{/if}<button type="button" onclick={() => jobs.discardUpload(job)}
							>{$t('flow.body.removeUpload')}</button
						>
					</div>{/if}
			</div>{/each}
		{#each jobsState.children as child (child.job.id)}{@const job = child.job}
			<div
				class="rounded-card border border-line p-3 text-sm"
				data-testid="flow-body-child-job"
				data-state={child.state.phase}
			>
				<p>{child.state.title}</p>
				<p role="status" class="text-xs text-fg-muted">{$t('flow.body.creatingPage')}</p>
				{#if child.state.error}<p role="alert" class="text-xs text-danger">
						{$t(BODY_ERROR_LABELS[child.state.error])}
					</p>
					<button type="button" disabled={readOnly} onclick={() => jobs.retryChild(job)}
						>{$t('flow.body.retry')}</button
					>{#if child.state.error === 'position'}<button
							type="button"
							disabled={readOnly}
							onmousedown={(event) => event.preventDefault()}
							onclick={() => {
								const target = bookmark();
								if (target) jobs.repositionChild(job, target);
							}}>{$t('flow.body.choosePosition')}</button
						>{/if}{/if}{#if !child.state.inserted}<button
						type="button"
						class="ml-3 text-xs"
						onclick={() => jobs.cancelChild(job)}>{$t('common.cancel')}</button
					>{/if}
			</div>{/each}
	</section>{/if}
<Modal
	open={!!draft}
	taskScoped={true}
	title={$t(
		BODY_MODAL_LABELS[
			draft?.mode === 'child'
				? 'child'
				: draft?.mode === 'link'
					? 'link'
					: draft?.mode === 'details'
						? 'details'
						: (draft?.kind ?? 'image')
		]
	)}
	onclose={close}
>
	{#if draft}<form
			class="space-y-4"
			onsubmit={(event) => {
				event.preventDefault();
				void submit();
			}}
			data-testid="flow-body-insert-form"
		>
			{#if draft.mode === 'child'}<label class="block text-sm"
					>{$t('flow.body.pageTitle')}<input
						class="mt-1 w-full rounded border px-3 py-2"
						value={draft.title}
						maxlength="200"
						oninput={(event) => update({ title: event.currentTarget.value })}
						required
					/></label
				>
				<p class="text-xs text-fg-muted">{$t('flow.body.childOwnership')}</p>
			{:else if draft.mode === 'link'}<label class="block text-sm"
					>{$t('flow.body.findPage')}
					<div class="mt-1 flex gap-2">
						<input class="min-w-0 flex-1 rounded border px-3 py-2" bind:value={query} /><button
							type="button"
							disabled={searching}
							onclick={search}>{$t('common.search')}</button
						>
					</div></label
				>
				<ul class="max-h-40 overflow-auto">
					{#each results as page (page.id)}<li>
							<button
								type="button"
								class="w-full rounded px-2 py-1 text-left text-sm hover:bg-surface"
								aria-pressed={draft.targetId === page.id}
								onclick={() => update({ targetId: page.id })}
								>{page.title || $t('flow.nav.untitled')}</button
							>
						</li>{/each}
				</ul>
				<label class="block text-sm"
					>{$t('flow.body.pageAddress')}<input
						class="mt-1 w-full rounded border px-3 py-2"
						value={draft.targetId}
						oninput={(event) => update({ targetId: event.currentTarget.value })}
						required
					/></label
				>
				<p class="text-xs text-fg-muted">{$t('flow.body.linkOwnership')}</p>
			{:else}
				{#if draft.kind === 'image'}<label class="block text-sm"
						>{$t('flow.body.altText')}<input
							class="mt-1 w-full rounded border px-3 py-2"
							value={draft.alt}
							maxlength="1024"
							oninput={(event) => update({ alt: event.currentTarget.value })}
						/></label
					>{/if}
				<label class="block text-sm"
					>{$t('flow.body.caption')}<input
						class="mt-1 w-full rounded border px-3 py-2"
						value={draft.caption}
						maxlength="1024"
						oninput={(event) => update({ caption: event.currentTarget.value })}
					/></label
				>
				{#if draft.mode !== 'details'}<label class="block text-sm"
						>{$t('flow.body.chooseFile')}<input
							type="file"
							class="mt-2 block w-full text-sm"
							accept={draft.kind === 'image' ? '.png,.jpg,.jpeg,.webp,.gif' : undefined}
							oninput={(event) => {
								const file = event.currentTarget.files?.[0];
								if (file) update({ file });
								event.currentTarget.value = '';
							}}
						/>{#if draft.file}<span class="mt-1 block text-xs text-fg-muted"
								>{draft.file.name} · {bytes(draft.file.size)}</span
							>{/if}</label
					>{/if}
			{/if}
			{#if error}<p role="alert" class="text-sm text-danger">{$t(BODY_ERROR_LABELS[error])}</p>
				{#if error === 'position'}<button
						type="button"
						class="text-sm underline"
						onclick={chooseCurrent}>{$t('flow.body.choosePosition')}</button
					>{/if}{/if}
			<div class="flex justify-end gap-3">
				<button type="button" onclick={close}>{$t('common.cancel')}</button><button
					type="submit"
					disabled={submitting || readOnly}
					class="rounded bg-primary rf-filled px-3 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted"
					>{$t(
						draft.mode === 'details'
							? 'common.save'
							: draft.mode === 'child'
								? 'flow.body.createPage'
								: draft.mode === 'link'
									? 'flow.body.insertLink'
									: 'flow.body.upload'
					)}</button
				>
			</div>
		</form>{/if}
</Modal>
