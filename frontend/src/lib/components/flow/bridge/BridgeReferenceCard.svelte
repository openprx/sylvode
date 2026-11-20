<script lang="ts">
	// One Forms Bridge card on a Page (`versions/v0.7-forms-bridge.md` "Web 交互"): a Reference is
	// a light card (title, type, permission badge, source link); an Embed also shows the target's
	// content when the request-time `permission_state` allows viewing, otherwise a placeholder.
	// Target reads come from `loadEmbedContent`, which asks Forms for nothing without `form.view`.
	import { onMount } from 'svelte';
	import { projectsApi } from '#lib/api/projects.js';
	import { t } from 'svelte-i18n';
	import { resolve } from '$app/paths';
	import {
		FORMS_EMBED_READS,
		PERMISSION_BADGE_KEYS,
		REFERENCE_MODE_KEYS,
		TARGET_TYPE_KEYS,
		canViewTarget,
		loadEmbedContent,
		permissionBadge,
		type AvailableReference,
		type EmbedContent,
		type ReferenceMode
	} from '#lib/flow/slash-bridge.js';

	interface Props {
		scopeId: string;
		reference: AvailableReference;
		mode: ReferenceMode;
		onremove: (referenceId: string) => void;
	}

	let { scopeId, reference, mode, onremove }: Props = $props();

	let sourceWorkspaceId = $state<string | null>(null);
	let content = $state<EmbedContent | null>(null);
	const badge = $derived(permissionBadge(reference.permission_state));
	const viewable = $derived(canViewTarget(reference.permission_state));

	onMount(async () => {
		content = await loadEmbedContent(reference, FORMS_EMBED_READS);
		if (content && (content.kind === 'form' || content.kind === 'record')) {
			const project = await projectsApi.get(content.projectId);
			sourceWorkspaceId = project.data?.workspace_id ?? null;
		}
	});

	const linkClass =
		'inline-flex min-h-11 items-center rounded px-2 text-sm font-medium text-fg underline';

	const badgeClass: Record<string, string> = {
		controlled: 'bg-surface text-fg',
		read_only: 'bg-surface text-fg',
		unconfigured: 'bg-warning-soft text-fg',
		no_access: 'bg-danger-soft text-danger'
	};
</script>

<article
	class="not-prose my-3 rounded-card border border-line bg-raised p-4"
	data-testid="flow-bridge-card"
	data-source-scope={scopeId}
	data-mode={mode}
	data-reference-id={reference.reference_id}
	data-target-type={reference.target_type}
	data-permission={badge}
>
	<div class="flex flex-wrap items-start gap-2">
		<div class="min-w-0 flex-1">
			<p class="text-xs font-medium uppercase tracking-wide text-fg-muted">
				{$t(REFERENCE_MODE_KEYS[mode])} · {$t(TARGET_TYPE_KEYS[reference.target_type])}
			</p>
			<h3
				class="truncate text-sm font-semibold text-fg"
				data-testid="flow-bridge-card-title"
			>
				{reference.title || $t('flow.bridge.card.untitled')}
			</h3>
		</div>
		<span
			class="rounded px-2 py-0.5 text-xs font-medium {badgeClass[badge]}"
			data-testid="flow-bridge-card-permission"
		>
			{$t(PERMISSION_BADGE_KEYS[badge])}
		</span>
	</div>

	{#if mode === 'embed'}
		<div class="mt-3" data-testid="flow-bridge-embed-body" aria-live="polite">
			{#if !viewable || content?.kind === 'restricted'}
				<p
					class="text-sm text-fg-muted"
					data-testid="flow-bridge-embed-restricted"
				>
					{$t('flow.bridge.card.restricted')}
				</p>
			{:else if !content}
				<p class="text-sm text-fg-muted">{$t('common.loading')}</p>
			{:else if content.kind === 'failed'}
				<p class="text-sm text-danger" role="alert">{$t('flow.bridge.card.contentFailed')}</p>
			{:else if content.kind === 'record'}
				{#if content.fields.length === 0}
					<p class="text-sm text-fg-muted">{$t('flow.bridge.card.emptyRecord')}</p>
				{:else}
					<dl class="grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[auto_1fr]">
						{#each content.fields as field (field.label)}
							<dt class="font-medium text-fg-muted">{field.label}</dt>
							<dd class="break-words text-fg">{field.value}</dd>
						{/each}
					</dl>
				{/if}
			{:else if content.kind === 'form'}
				<p class="text-sm text-fg">
					{content.description || $t('flow.bridge.card.noDescription')}
				</p>
				<p class="mt-1 text-xs text-fg-muted">
					{$t('flow.bridge.card.fieldCount', { values: { count: content.fieldCount } })}
				</p>
			{/if}
		</div>
		<p class="mt-2 text-xs text-fg-muted">
			{$t('flow.canvas.slashMenu.embedNote')}
		</p>
	{:else}
		<p class="mt-2 text-xs text-fg-muted">
			{$t('flow.canvas.slashMenu.referenceNote')}
		</p>
	{/if}

	<div class="mt-3 flex flex-wrap items-center gap-2">
		{#if content?.kind === 'record' && sourceWorkspaceId}
			<a
				href={resolve(
					'/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]',
					{
						workspaceId: sourceWorkspaceId!,
						projectId: content.projectId,
						recordId: reference.target_id
					}
				)}
				class={linkClass}
				data-testid="flow-bridge-card-source"
			>
				{$t('flow.bridge.card.openSource')}
			</a>
		{:else if content?.kind === 'form' && sourceWorkspaceId}
			<!-- The path is resolve()d; only the ?form= query that selects the form is appended. -->
			<!-- eslint-disable svelte/no-navigation-without-resolve -->
			<a
				href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
					workspaceId: sourceWorkspaceId!,
					projectId: content.projectId
				}) + `?form=${encodeURIComponent(content.formId)}`}
				class={linkClass}
				data-testid="flow-bridge-card-source"
			>
				{$t('flow.bridge.card.openSource')}
			</a>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{/if}
		<button
			type="button"
			class="ml-auto inline-flex min-h-11 items-center rounded px-3 text-sm font-medium text-danger hover:bg-danger-soft"
			data-testid="flow-bridge-card-remove"
			onclick={() => onremove(reference.reference_id)}
		>
			{$t('flow.bridge.card.remove')}
		</button>
	</div>
</article>
