<script lang="ts">
	import { onDestroy, tick } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import type { FlowConversionPreview } from '#lib/api/flow.js';
	import type { FlowObjectRepository } from '#lib/flow/object-repository.js';
	import type { FlowCommandService } from '#lib/flow/command-service.js';
	import {
		listWorkspaceProjects,
		newIdempotencyKey,
		type WorkspaceProjectOption
	} from '#lib/flow/package-export.js';
	import { classifyConvertFailure, parseConversionWarnings } from '#lib/flow/convert-wizard.js';

	let {
		workspaceId,
		objectId,
		repository,
		commands
	}: {
		workspaceId: string;
		objectId: string;
		repository: Pick<FlowObjectRepository, 'get'>;
		commands: Pick<FlowCommandService, 'convertPreview' | 'convertCommit'>;
	} = $props();

	const uid = $props.id();
	const typeKeys = {
		text: 'flow.createForm.types.text',
		textarea: 'flow.createForm.types.textarea',
		integer: 'flow.createForm.types.integer',
		number: 'flow.createForm.types.number',
		date: 'flow.createForm.types.date',
		boolean: 'flow.createForm.types.boolean'
	};
	const types = Object.keys(typeKeys) as Array<keyof typeof typeKeys>;
	type FormField = {
		id: number;
		key: string;
		label: string;
		type: (typeof types)[number];
		required: boolean;
	};
	let rowId = 0;
	function newField(): FormField {
		return { id: ++rowId, key: '', label: '', type: 'text', required: false };
	}
	let projects = $state<WorkspaceProjectOption[]>([]);
	let projectsLoading = $state(true);
	let projectId = $state('');
	let name = $state('');
	let key = $state('');
	let fields = $state([newField()]);
	let error = $state('');
	let busy = $state(false);
	let preview = $state.raw<{
		response: FlowConversionPreview;
		name: string;
		key: string;
		project: string;
		fields: FormField[];
	} | null>(null);
	let reviewHeading = $state<HTMLElement | null>(null);
	let schemaHeading = $state<HTMLElement | null>(null);
	let confirmed = $state(false);
	let previewIntent: { signature: string; key: string } | null = null;
	let commitKey = '';
	let alive = true;
	const input =
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm dark:border-slate-600 dark:bg-slate-900';
	const button =
		'min-h-11 rounded-md border border-slate-300 px-3 text-sm disabled:opacity-50 dark:border-slate-600';
	onDestroy(() => {
		alive = false;
	});

	$effect(() => {
		const id = workspaceId;
		let current = true;
		projectsLoading = true;
		void listWorkspaceProjects(id).then((items) => {
			if (!current) return;
			projects = items;
			projectId = items.length === 1 ? items[0].id : '';
			projectsLoading = false;
		});
		return () => {
			current = false;
		};
	});

	async function edit() {
		confirmed = false;
		preview = null;
		error = '';
		await tick();
		schemaHeading?.focus();
	}

	async function prepare(event: SubmitEvent) {
		event.preventDefault();
		if (busy || !projectId) return;
		const schemaFields = fields.map(({ key, label, type, required }) => ({
			key: key.trim(),
			label: label.trim(),
			type,
			required
		}));
		if (
			new Set(schemaFields.map((field) => field.key)).size !== schemaFields.length ||
			!name.trim() ||
			schemaFields.some((field) => !field.label)
		) {
			error = 'flow.createForm.invalidFields';
			return;
		}
		busy = true;
		error = '';
		const mapping = {
			target_project_id: projectId,
			key: key.trim(),
			name: name.trim(),
			schema: { version: 'openpr.form.schema.v1', fields: schemaFields }
		};
		const proposedFields = $state.snapshot(fields);
		const project = projects.find((item) => item.id === projectId)?.label ?? projectId;
		try {
			const source = await repository.get(objectId);
			if (!alive) return;
			if (source.code !== 0 || !source.data || source.data.object_type !== 'page') {
				error = 'flow.bridge.convert.error.sourceUnavailable';
				return;
			}
			const signature = JSON.stringify([source.data.frontier, mapping]);
			if (previewIntent?.signature !== signature)
				previewIntent = { signature, key: newIdempotencyKey() };
			const result = await commands.convertPreview({
				source_object_id: objectId,
				source_frontier: source.data.frontier,
				destination_workspace_id: workspaceId,
				destination_project_id: mapping.target_project_id,
				target_type: 'form',
				mapping,
				idempotency_key: previewIntent.key
			});
			if (!alive) return;
			if (result.code !== 0 || !result.data) {
				error =
					result.code !== 0
						? classifyConvertFailure(result).messageKey
						: 'flow.createForm.previewFailed';
				return;
			}
			preview = {
				response: result.data,
				name: mapping.name,
				key: mapping.key,
				project,
				fields: proposedFields
			};
			confirmed = false;
			previewIntent = null;
			commitKey = newIdempotencyKey();
			await tick();
			reviewHeading?.focus();
		} catch {
			if (alive) error = 'flow.createForm.previewFailed';
		} finally {
			if (alive) busy = false;
		}
	}

	async function commit() {
		if (!preview || busy || !confirmed) return;
		busy = true;
		error = '';
		const current = preview.response;
		if (Date.parse(current.expires_at) <= Date.now()) {
			busy = false;
			error = 'flow.createForm.expired';
			return;
		}
		try {
			const result = await commands.convertCommit({
				preview_id: current.preview_id,
				source_frontier: current.source_frontier,
				target_schema_version: current.target_schema_version,
				idempotency_key: commitKey,
				confirm: true
			});
			if (!alive) return;
			const failure = result.code !== 0 ? classifyConvertFailure(result) : null;
			const jobId = result.data?.job_id ?? failure?.jobId;
			if (jobId) {
				await goto(resolve('/(app)/flow/conversions/[jobId]', { jobId }));
				return;
			}
			error = failure?.messageKey ?? 'flow.createForm.commitFailed';
		} catch {
			if (alive) error = 'flow.createForm.commitFailed';
		} finally {
			if (alive) busy = false;
		}
	}
</script>

<section
	class="space-y-4 rounded-lg border p-4 dark:border-slate-700"
	aria-busy={busy}
	data-testid="flow-create-form"
>
	{#if !preview}
		<h2 class="text-base font-semibold" tabindex="-1" bind:this={schemaHeading}>
			{$t('flow.createForm.definition')}
		</h2>
		<p class="text-sm">{$t('flow.createForm.definitionNote')}</p>
		{#if projectsLoading}<p role="status">{$t('common.loading')}</p>
		{:else if !projects.length}<p role="status">{$t('flow.createForm.noProject')}</p>
		{:else}
			<form onsubmit={prepare} class="space-y-4">
				<fieldset disabled={busy} class="space-y-4">
					<label class="block space-y-1 text-sm">
						<span>{$t('flow.createForm.project')}</span>
						<select class={input} bind:value={projectId} required>
							<option value="">{$t('flow.graphPackage.chooseProject')}</option>
							{#each projects as project (project.id)}<option value={project.id}
									>{project.label}</option
								>{/each}
						</select>
					</label>
					<label class="block space-y-1 text-sm">
						<span>{$t('flow.createForm.name')}</span>
						<input class={input} bind:value={name} required maxlength="120" />
					</label>
					<label class="block space-y-1 text-sm">
						<span>{$t('flow.createForm.key')}</span>
						<input
							class={input}
							bind:value={key}
							required
							maxlength="64"
							pattern="[a-z][a-z0-9_]*"
							aria-describedby={`${uid}-key`}
						/>
						<span id={`${uid}-key`} class="block text-xs">{$t('flow.createForm.keyHint')}</span>
					</label>
					{#each fields as field, index (field.id)}
						<fieldset class="space-y-2 rounded border p-3 dark:border-slate-700">
							<legend class="px-2 text-sm font-medium"
								>{$t('flow.createForm.field', { values: { index: index + 1 } })}</legend
							>
							<div class="grid gap-3 sm:grid-cols-2">
								<label class="block space-y-1 text-sm"
									><span>{$t('flow.createForm.fieldLabel')}</span>
									<input class={input} required maxlength="120" bind:value={field.label} /></label
								>
								<label class="block space-y-1 text-sm"
									><span>{$t('flow.createForm.fieldKey')}</span>
									<input
										class={input}
										required
										maxlength="64"
										pattern="[a-z][a-z0-9_]*"
										bind:value={field.key}
										aria-describedby={`${uid}-key`}
									/></label
								>
							</div>
							<label class="block space-y-1 text-sm"
								><span>{$t('flow.createForm.fieldType')}</span>
								<select class={input} bind:value={field.type}>
									{#each types as type (type)}<option value={type}>{$t(typeKeys[type])}</option
										>{/each}
								</select>
							</label>
							<label class="flex min-h-11 items-center gap-2 text-sm"
								><input type="checkbox" bind:checked={field.required} />{$t(
									'flow.createForm.required'
								)}</label
							>
							{#if fields.length > 1}<button
									type="button"
									class={button}
									onclick={() => (fields = fields.filter((item) => item.id !== field.id))}
									>{$t('flow.createForm.removeField')}</button
								>{/if}
						</fieldset>
					{/each}
					<div class="flex flex-wrap gap-2">
						<button
							class={button}
							type="button"
							disabled={fields.length >= 32}
							onclick={() => (fields = [...fields, newField()])}
							>{$t('flow.createForm.addField')}</button
						>
						<button class={button} type="submit">{$t('flow.createForm.preview')}</button>
					</div>
				</fieldset>
			</form>
		{/if}
	{:else}
		<h2 class="text-lg font-semibold" tabindex="-1" bind:this={reviewHeading}>
			{$t('flow.createForm.review')}
		</h2>
		<p class="text-sm font-medium">{preview.name} · {preview.project}</p>
		<p class="break-all text-sm">{preview.key}</p>
		<ul class="space-y-2 rounded border p-3 dark:border-slate-700">
			{#each preview.fields as field (field.id)}
				<li class="text-sm">
					{field.label} ({field.key}) · {$t(typeKeys[field.type])}
					{#if field.required}· {$t('flow.createForm.required')}{/if}
				</li>
			{/each}
		</ul>
		{#each parseConversionWarnings(preview.response.warnings) as warning, index (index)}
			<p class="text-sm">
				{$t(warning.messageKey, {
					values: {
						field:
							preview.fields.find((field) => field.key === warning.field)?.label ?? warning.field
					}
				})}
			</p>
		{/each}
		<label class="flex min-h-11 items-center gap-2 text-sm">
			<input type="checkbox" disabled={busy} bind:checked={confirmed} />{$t(
				'flow.createForm.confirm'
			)}
		</label>
		<div class="flex flex-wrap gap-2">
			<button type="button" class={button} disabled={busy || !confirmed} onclick={commit}
				>{$t('flow.createForm.create')}</button
			>
			<button type="button" class={button} disabled={busy} onclick={edit}
				>{$t('flow.createForm.edit')}</button
			>
		</div>
	{/if}
	{#if busy}<p class="text-sm" role="status">{$t('common.loading')}</p>{/if}
	{#if error}<p class="text-sm text-danger" role="alert">{$t(error)}</p>{/if}
</section>
