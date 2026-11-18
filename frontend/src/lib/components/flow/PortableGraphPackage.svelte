<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import {
		flowApi,
		type FlowObjectView,
		type PortableGraphPreview,
		type PortableGraphReceipt
	} from '#lib/api/flow.js';
	import { formsApi, type UniversalForm } from '#lib/api/forms.js';
	import {
		listWorkspaceProjects,
		newIdempotencyKey,
		type WorkspaceProjectOption
	} from '#lib/flow/package-export.js';
	import { MAX_PORTABLE_PAGE_BYTES, portablePageFailure } from '#lib/flow/portable-page-package.js';

	let {
		object,
		canImport,
		exportReady,
		oncreated
	}: {
		object: FlowObjectView;
		canImport: boolean;
		exportReady: boolean;
		oncreated?: (objects: Array<{ id: string; parentId: string }>) => void;
	} = $props();
	const uid = $props.id();
	let open = $state(false);
	let busy = $state<'export' | 'preview' | 'import' | null>(null);
	let error = $state('');
	let pickerError = $state(false);
	let query = $state('');
	let loading = $state(false);
	let searchGeneration = 0;
	let alive = true;
	const maxPages = 16;
	const maxForms = 16;
	let pages = $state<FlowObjectView[]>([]);
	let chosenPages = $state<FlowObjectView[]>([]);
	let cursor = $state<string | undefined>();
	let projects = $state<WorkspaceProjectOption[]>([]);
	let projectId = $state('');
	let formPage = $state(1);
	let formPages = $state(1);
	let forms = $state<UniversalForm[]>([]);
	let chosenForms = $state<UniversalForm[]>([]);
	let includeHistory = $state(false);
	type Review = {
		archive: Blob;
		preview: PortableGraphPreview;
		mode: 'export' | 'import';
		key: string;
	};
	let review = $state.raw<Review | null>(null);
	let receipt = $state.raw<PortableGraphReceipt | null>(null);
	let heading = $state<HTMLElement | null>(null);
	const field =
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-900';
	const button =
		'min-h-11 rounded-md border border-slate-300 px-3 text-sm underline-offset-2 hover:underline focus-visible:ring-2 disabled:opacity-50 dark:border-slate-600';
	onDestroy(() => {
		alive = false;
	});

	async function discover(append = false) {
		if (busy) return;
		const request = ++searchGeneration;
		loading = true;
		pickerError = false;
		try {
			const result = await flowApi.listObjects(object.scope_id, {
				object_type: 'page',
				q: query,
				cursor: append ? cursor : undefined,
				limit: 30
			});
			if (!alive || request !== searchGeneration) return;
			if (result.code !== 0 || !result.data) {
				pickerError = true;
				return;
			}
			const found = result.data.items.filter(
				(item) =>
					item.id !== object.id &&
					item.scope_id === object.scope_id &&
					item.lifecycle_status === 'active' &&
					item.permitted_actions?.includes('view')
			);
			pages = [
				...new Map([...(append ? pages : []), ...found].map((item) => [item.id, item])).values()
			];
			cursor = result.data.next_cursor;
		} catch {
			if (alive && request === searchGeneration) pickerError = true;
		} finally {
			if (alive && request === searchGeneration) loading = false;
		}
	}

	async function start() {
		open = !open;
		if (!open || pages.length) return;
		void discover();
		if (object.workspace_id) projects = await listWorkspaceProjects(object.workspace_id);
	}
	async function loadForms(project: string, targetPage = 1) {
		projectId = project;
		formPage = targetPage;
		forms = [];
		if (!project) return;
		pickerError = false;
		try {
			const result = await formsApi.list(project, { page: targetPage, per_page: 30 });
			if (!alive || projectId !== project || formPage !== targetPage) return;
			if (result.code !== 0 || !result.data) {
				pickerError = true;
				return;
			}
			forms = result.data.items.filter((form) => !form.archived_at);
			formPages = result.data.total_pages;
		} catch {
			if (alive && projectId === project) pickerError = true;
		}
	}
	function pickPage(item: FlowObjectView, checked: boolean) {
		chosenPages = checked
			? [...chosenPages, item]
			: chosenPages.filter((value) => value.id !== item.id);
	}
	function pickForm(item: UniversalForm, checked: boolean) {
		chosenForms = checked
			? [...chosenForms, item]
			: chosenForms.filter((value) => value.id !== item.id);
	}
	function displayTitle(id: string, kind: 'page' | 'form') {
		return kind === 'page'
			? review?.preview.pages.find((item) => item.source_object_id === id)?.title || id
			: review?.preview.forms.find((item) => item.source_form_id === id)?.name || id;
	}
	async function readReview(archive: Blob, mode: Review['mode']) {
		const result = await flowApi.previewPortableGraph(object.id, archive, mode);
		if (!alive) return;
		if (result.code !== 0 || !result.data) {
			error =
				mode === 'import' &&
				(!object.workspace_id || !object.project_id) &&
				typeof result.details === 'object' &&
				result.details !== null &&
				'reason' in result.details &&
				result.details.reason === 'portable_graph_forms_require_project'
					? 'flow.graphPackage.formsRequireProject'
					: portablePageFailure(result);
			return;
		}
		review = { archive, mode, preview: result.data, key: newIdempotencyKey() };
		await tick();
		heading?.focus();
	}
	async function prepare() {
		if (busy || !exportReady) return;
		busy = 'export';
		review = null;
		receipt = null;
		error = '';
		try {
			const result = await flowApi.exportPortableGraph(object.id, {
				page_ids: [object.id, ...chosenPages.map((item) => item.id)],
				form_ids: chosenForms.map((item) => item.id),
				include_history: includeHistory
			});
			if (!alive) return;
			if (result.code !== 0 || !result.data) error = portablePageFailure(result);
			else if (!result.data.headers.get('Content-Type')?.startsWith('application/zip'))
				error = 'flow.portablePackage.invalid';
			else await readReview(result.data.blob, 'export');
		} catch {
			if (alive) error = 'flow.portablePackage.failed';
		} finally {
			if (alive) busy = null;
		}
	}
	function download(archive: Blob) {
		const url = URL.createObjectURL(archive);
		const anchor = document.createElement('a');
		anchor.href = url;
		anchor.download = `${object.id}.sylvode-pages.zip`;
		document.body.appendChild(anchor);
		anchor.click();
		anchor.remove();
		setTimeout(() => URL.revokeObjectURL(url), 60_000);
	}
	async function chooseFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		input.value = '';
		if (!file || busy || !canImport) return;
		review = null;
		receipt = null;
		error = '';
		if (file.size > MAX_PORTABLE_PAGE_BYTES) {
			error = 'flow.portablePackage.tooLarge';
			return;
		}
		busy = 'preview';
		try {
			await readReview(file, 'import');
		} catch {
			if (alive) error = 'flow.portablePackage.invalid';
		} finally {
			if (alive) busy = null;
		}
	}
	async function importGraph() {
		if (!review || review.mode !== 'import' || busy || !canImport || receipt) return;
		busy = 'import';
		error = '';
		try {
			const result = await flowApi.importPortableGraph(
				object.id,
				review.archive,
				review.preview.package_sha256,
				review.key
			);
			if (!alive) return;
			if (result.code !== 0 || !result.data) error = portablePageFailure(result);
			else {
				receipt = result.data;
				const mappings = new Map(
					receipt.pages.map((item) => [item.source_object_id, item.object_id])
				);
				oncreated?.(
					receipt.pages.map((item) => {
						const source = review?.preview.pages.find(
							(page) => page.source_object_id === item.source_object_id
						);
						const parentId = source?.source_parent_id
							? mappings.get(source.source_parent_id)
							: undefined;
						return { id: item.object_id, parentId: parentId ?? object.id };
					})
				);
				await tick();
				heading?.focus();
			}
		} catch {
			if (alive) error = 'flow.portablePackage.failed';
		} finally {
			if (alive) busy = null;
		}
	}
	function pageLabel(item: FlowObjectView) {
		const parent =
			item.parent_id === object.id ? object : pages.find((page) => page.id === item.parent_id);
		const title = item.title || $t('flow.nav.untitled');
		return `${title} · ${parent?.title ? `${parent.title} · ` : ''}${item.id.slice(0, 8)}`;
	}

	function formLabel(form: UniversalForm) {
		const project = projects.find((item) => item.id === form.project_id);
		return `${form.name} · ${project?.label ? `${project.label} · ` : ''}${form.id.slice(0, 8)}`;
	}

	function pageHref(id: string) {
		return page.params.workspaceId
			? resolve('/(app)/workspace/[workspaceId]/flow/[objectId]', {
					workspaceId: page.params.workspaceId,
					objectId: id
				})
			: resolve('/(app)/flow/[objectId]', { objectId: id });
	}
</script>

<section
	class="my-4 space-y-3 border-t pt-4 dark:border-slate-700"
	data-testid="flow-graph-package"
>
	<h3 class="text-sm font-semibold">{$t('flow.graphPackage.title')}</h3>
	<button
		type="button"
		class={button}
		aria-expanded={open}
		aria-controls={`${uid}-body`}
		onclick={start}
	>
		{$t(open ? 'flow.graphPackage.hide' : 'flow.graphPackage.show')}
	</button>
	<div id={`${uid}-body`} hidden={!open} aria-busy={busy !== null} class="space-y-4">
		<fieldset disabled={busy !== null} class="space-y-2">
			<legend class="text-sm font-medium">{$t('flow.graphPackage.selectPages')}</legend>
			<p class="text-xs">
				{$t('flow.graphPackage.selectionCount', {
					values: { chosen: chosenPages.length + 1, max: maxPages }
				})}
			</p>
			<label class="block text-sm" for={`${uid}-query`}>{$t('flow.graphPackage.search')}</label>
			<div class="flex gap-2">
				<input
					id={`${uid}-query`}
					class={field}
					type="search"
					bind:value={query}
					onkeydown={(event) => {
						if (event.key === 'Enter' && !event.isComposing) {
							event.preventDefault();
							void discover();
						}
					}}
				/>
				<button type="button" class={button} onclick={() => discover()}
					>{$t('flow.graphPackage.searchAction')}</button
				>
			</div>
			{#if loading}<p role="status">{$t('common.loading')}</p>{/if}
			<label class="flex items-center gap-2 text-sm"
				><input type="checkbox" checked disabled />{object.title || $t('flow.nav.untitled')}</label
			>
			<ul class="max-h-48 space-y-2 overflow-y-auto">
				{#each pages as item (item.id)}
					<li>
						<label class="flex items-center gap-2 text-sm">
							<input
								type="checkbox"
								data-object-id={item.id}
								checked={chosenPages.some((value) => value.id === item.id)}
								disabled={chosenPages.length + 1 >= maxPages &&
									!chosenPages.some((value) => value.id === item.id)}
								onchange={(event) => pickPage(item, event.currentTarget.checked)}
							/>
							{pageLabel(item)}
						</label>
					</li>
				{/each}
			</ul>
			{#if cursor}<button
					type="button"
					class={button}
					disabled={loading}
					onclick={() => discover(true)}>{$t('flow.history.loadMore')}</button
				>{/if}
			{#if chosenPages.length}
				<ul class="space-y-1 text-sm">
					{#each chosenPages as item (item.id)}
						<li>
							{pageLabel(item)}
							<button
								class="min-h-8 px-2 underline"
								type="button"
								onclick={() => pickPage(item, false)}
								aria-label={$t('flow.graphPackage.removeItem', {
									values: { name: pageLabel(item) }
								})}>{$t('flow.graphPackage.remove')}</button
							>
						</li>
					{/each}
				</ul>
			{/if}
		</fieldset>
		{#if object.workspace_id && projects.length}
			<fieldset disabled={busy !== null} class="space-y-2">
				<legend class="text-sm font-medium">{$t('flow.graphPackage.selectForms')}</legend>
				<p class="text-xs">
					{$t('flow.graphPackage.selectionCount', {
						values: { chosen: chosenForms.length, max: maxForms }
					})}
				</p>
				<label class="block text-sm" for={`${uid}-project`}
					>{$t('flow.graphPackage.formsProject')}</label
				>
				<select
					id={`${uid}-project`}
					class={field}
					value={projectId}
					onchange={(event) => loadForms(event.currentTarget.value)}
				>
					<option value="">{$t('flow.graphPackage.chooseProject')}</option>
					{#each projects as project (project.id)}<option value={project.id}>{project.label}</option
						>{/each}
				</select>
				<ul class="max-h-48 space-y-2 overflow-y-auto">
					{#each forms as form (form.id)}
						<li>
							<label class="flex items-center gap-2 text-sm">
								<input
									type="checkbox"
									data-form-id={form.id}
									checked={chosenForms.some((value) => value.id === form.id)}
									disabled={chosenForms.length >= maxForms &&
										!chosenForms.some((value) => value.id === form.id)}
									onchange={(event) => pickForm(form, event.currentTarget.checked)}
								/>
								{formLabel(form)}
							</label>
						</li>
					{/each}
				</ul>
				{#if projectId && formPage > 1}<button
						type="button"
						class={button}
						onclick={() => loadForms(projectId, formPage - 1)}
						>{$t('flow.graphPackage.previous')}</button
					>{/if}
				{#if projectId && formPage < formPages}<button
						type="button"
						class={button}
						onclick={() => loadForms(projectId, formPage + 1)}
						>{$t('flow.graphPackage.next')}</button
					>{/if}
				{#if chosenForms.length}
					<ul class="space-y-1 text-sm">
						{#each chosenForms as form (form.id)}
							<li>
								{formLabel(form)}<button
									class="min-h-8 px-2 underline"
									type="button"
									onclick={() => pickForm(form, false)}
									aria-label={$t('flow.graphPackage.removeItem', {
										values: { name: formLabel(form) }
									})}>{$t('flow.graphPackage.remove')}</button
								>
							</li>
						{/each}
					</ul>
				{/if}
			</fieldset>
		{/if}
		{#if pickerError}<p role="alert" class="text-sm text-red-700 dark:text-red-300">
				{$t('flow.graphPackage.discoveryFailed')}
			</p>{/if}
		<label class="flex items-center gap-2 text-sm"
			><input type="checkbox" bind:checked={includeHistory} disabled={busy !== null} />{$t(
				'flow.graphPackage.includeHistory'
			)}</label
		>
		<button type="button" class={button} disabled={busy !== null || !exportReady} onclick={prepare}
			>{$t('flow.graphPackage.prepare')}</button
		>
		{#if !exportReady}<p class="text-sm">{$t('flow.history.restoreWait')}</p>{/if}
		{#if canImport}<label class="block space-y-2 text-sm">
				<span>{$t('flow.graphPackage.chooseZip')}</span>
				<input
					type="file"
					class="block max-w-full"
					accept=".zip,application/zip"
					disabled={busy !== null}
					onchange={chooseFile}
				/>
			</label>{/if}
		{#if busy}<p class="text-sm" role="status">{$t('common.loading')}</p>{/if}
		{#if error}<p class="text-sm text-red-700 dark:text-red-300" role="alert">{$t(error)}</p>{/if}
		{#if review && !receipt}
			<section
				class="space-y-3 rounded-md border p-3 text-sm"
				data-testid="flow-graph-package-review"
				aria-labelledby={`${uid}-review`}
			>
				<h4 class="font-semibold" id={`${uid}-review`} tabindex="-1" bind:this={heading}>
					{$t('flow.graphPackage.review')}
				</h4>
				<p>
					{$t(
						review.preview.include_history
							? 'flow.graphPackage.historyIncluded'
							: 'flow.graphPackage.currentOnly'
					)}
				</p>
				<ul class="space-y-2">
					{#each review.preview.pages as item (item.source_object_id)}
						<li>
							<strong>{item.title || $t('flow.nav.untitled')}</strong>
							<span class="block"
								>{$t('flow.graphPackage.pageDetails', {
									values: { assets: item.asset_count, versions: item.version_count }
								})}</span
							>
							{#if item.source_parent_id}
								<span class="block"
									>{$t('flow.graphPackage.parent', {
										values: { name: displayTitle(item.source_parent_id, 'page') }
									})}</span
								>
							{/if}
							{#if review.preview.include_history && item.source_sequences.length > 0}
								<span class="block"
									>{$t('flow.graphPackage.sourceVersions', {
										values: { versions: item.source_sequences.join(', ') }
									})}</span
								>
							{/if}
							{#if item.excerpts?.length}
								<p class="mt-2 font-medium">{$t('flow.graphPackage.headExcerpts')}</p>
								<ul class="my-2 list-inside list-disc space-y-1">
									{#each item.excerpts as excerpt, index (index)}
										<li class="break-words">{excerpt}</li>
									{/each}
								</ul>
							{/if}
						</li>
					{/each}
				</ul>
				{#if review.preview.forms.length}
					<h5 class="font-medium">{$t('flow.graphPackage.forms')}</h5>
					<ul class="list-inside list-disc">
						{#each review.preview.forms as form (form.source_form_id)}<li>{form.name}</li>{/each}
					</ul>
				{/if}
				{#if review.preview.references?.length}
					<h5 class="font-medium">{$t('flow.graphPackage.references')}</h5>
					<ul class="space-y-2">
						{#each review.preview.references as reference, index (index)}
							<li class="break-all">
								{$t(
									reference.kind === 'child_page'
										? 'flow.graphPackage.remapChild'
										: reference.kind === 'form'
											? 'flow.graphPackage.remapForm'
											: 'flow.graphPackage.remap',
									{
										values: {
											from: displayTitle(reference.source_object_id, 'page'),
											to:
												reference.kind === 'form'
													? displayTitle(reference.target_form_id, 'form')
													: displayTitle(reference.target_object_id, 'page')
										}
									}
								)}
							</li>
						{/each}
					</ul>
				{/if}
				{#if review.mode === 'export'}
					<button type="button" class={button} onclick={() => review && download(review.archive)}
						>{$t('flow.graphPackage.download')}</button
					>
				{:else}
					<p>
						{$t('flow.graphPackage.importDestination', {
							values: { name: object.title || $t('flow.nav.untitled') }
						})}
					</p>
					<button
						type="button"
						class={button}
						disabled={busy !== null || !canImport}
						onclick={importGraph}>{$t('flow.graphPackage.create')}</button
					>
				{/if}
			</section>
		{:else if receipt}
			<section
				class="space-y-2 rounded-md border p-3 text-sm"
				data-testid="flow-graph-package-receipt"
				aria-labelledby={`${uid}-receipt`}
			>
				<h4 class="font-semibold" id={`${uid}-receipt`} tabindex="-1" bind:this={heading}>
					{$t('flow.graphPackage.imported')}
				</h4>
				<ul class="space-y-2">
					{#each receipt.pages as item (item.object_id)}
						<li>
							<a class="inline-flex min-h-11 items-center underline" href={pageHref(item.object_id)}
								>{displayTitle(item.source_object_id, 'page')}</a
							>
							{#if receipt.include_history}
								<span class="block">{$t('flow.graphPackage.openHistoryAfterImport')}</span>
							{/if}
						</li>
					{/each}
					{#each receipt.forms as item (item.form_id)}
						<li>
							{#if object.workspace_id && object.project_id}<a
									class="inline-flex min-h-11 items-center underline"
									href={resolve(
										`/(app)/workspace/[workspaceId]/projects/[projectId]/forms?form=${encodeURIComponent(item.form_id)}`,
										{ workspaceId: object.workspace_id, projectId: object.project_id }
									)}>{displayTitle(item.source_form_id, 'form')}</a
								>
							{:else}{displayTitle(item.source_form_id, 'form')}{/if}
						</li>
					{/each}
				</ul>
			</section>
		{/if}
	</div>
</section>
