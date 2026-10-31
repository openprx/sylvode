<script lang="ts">
	// One Forms Bridge card on a Page (`versions/v0.7-forms-bridge.md` "Web 交互"): a Reference is
	// a light card (title, type, permission badge, source link); an Embed also shows the target's
	// content when the request-time `permission_state` allows viewing, otherwise a placeholder.
	// Target reads come from `loadEmbedContent`, which asks Forms for nothing without `form.view`.
	import { onMount } from 'svelte';
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
		workspaceId: string;
		reference: AvailableReference;
		mode: ReferenceMode;
		onremove: (referenceId: string) => void;
	}

	let { workspaceId, reference, mode, onremove }: Props = $props();

	let content = $state<EmbedContent | null>(null);
	const badge = $derived(permissionBadge(reference.permission_state));
	const viewable = $derived(canViewTarget(reference.permission_state));

	onMount(async () => {
		content = await loadEmbedContent(reference, FORMS_EMBED_READS);
	});

	const linkClass =
		'inline-flex min-h-11 items-center rounded px-2 text-sm font-medium text-blue-700 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-blue-300';

	const badgeClass: Record<string, string> = {
		controlled: 'bg-emerald-100 text-emerald-900 dark:bg-emerald-900 dark:text-emerald-100',
		read_only: 'bg-slate-100 text-slate-800 dark:bg-slate-800 dark:text-slate-100',
		unconfigured: 'bg-amber-100 text-amber-900 dark:bg-amber-900 dark:text-amber-100',
		no_access: 'bg-red-100 text-red-900 dark:bg-red-900 dark:text-red-100'
	};
</script>

<article
	class="not-prose my-3 rounded-lg border border-slate-200 bg-white p-4 dark:border-slate-700 dark:bg-slate-900"
	data-testid="flow-bridge-card"
	data-mode={mode}
	data-reference-id={reference.reference_id}
	data-target-type={reference.target_type}
	data-permission={badge}
>
	<div class="flex flex-wrap items-start gap-2">
		<div class="min-w-0 flex-1">
			<p class="text-xs font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">
				{$t(REFERENCE_MODE_KEYS[mode])} · {$t(TARGET_TYPE_KEYS[reference.target_type])}
			</p>
			<h3
				class="truncate text-sm font-semibold text-slate-900 dark:text-slate-100"
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
					class="text-sm text-slate-500 dark:text-slate-400"
					data-testid="flow-bridge-embed-restricted"
				>
					{$t('flow.bridge.card.restricted')}
				</p>
			{:else if !content}
				<p class="text-sm text-slate-500">{$t('common.loading')}</p>
			{:else if content.kind === 'failed'}
				<p class="text-sm text-red-600" role="alert">{$t('flow.bridge.card.contentFailed')}</p>
			{:else if content.kind === 'record'}
				{#if content.fields.length === 0}
					<p class="text-sm text-slate-500">{$t('flow.bridge.card.emptyRecord')}</p>
				{:else}
					<dl class="grid grid-cols-1 gap-x-4 gap-y-1 text-sm sm:grid-cols-[auto_1fr]">
						{#each content.fields as field (field.label)}
							<dt class="font-medium text-slate-600 dark:text-slate-300">{field.label}</dt>
							<dd class="break-words text-slate-900 dark:text-slate-100">{field.value}</dd>
						{/each}
					</dl>
				{/if}
			{:else if content.kind === 'form'}
				<p class="text-sm text-slate-700 dark:text-slate-200">
					{content.description || $t('flow.bridge.card.noDescription')}
				</p>
				<p class="mt-1 text-xs text-slate-500">
					{$t('flow.bridge.card.fieldCount', { values: { count: content.fieldCount } })}
				</p>
			{/if}
		</div>
		<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
			{$t('flow.canvas.slashMenu.embedNote')}
		</p>
	{:else}
		<p class="mt-2 text-xs text-slate-500 dark:text-slate-400">
			{$t('flow.canvas.slashMenu.referenceNote')}
		</p>
	{/if}

	<div class="mt-3 flex flex-wrap items-center gap-2">
		{#if content?.kind === 'record'}
			<a
				href={resolve(
					'/(app)/workspace/[workspaceId]/projects/[projectId]/forms/records/[recordId]',
					{ workspaceId, projectId: content.projectId, recordId: reference.target_id }
				)}
				class={linkClass}
				data-testid="flow-bridge-card-source"
			>
				{$t('flow.bridge.card.openSource')}
			</a>
		{:else if content?.kind === 'form'}
			<!-- The path is resolve()d; only the ?form= query that selects the form is appended. -->
			<!-- eslint-disable svelte/no-navigation-without-resolve -->
			<a
				href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]/forms', {
					workspaceId,
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
			class="ml-auto inline-flex min-h-11 items-center rounded px-3 text-sm font-medium text-red-700 hover:bg-red-50 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:text-red-300 dark:hover:bg-red-950"
			data-testid="flow-bridge-card-remove"
			onclick={() => onremove(reference.reference_id)}
		>
			{$t('flow.bridge.card.remove')}
		</button>
	</div>
</article>
