<svelte:options runes />

<script lang="ts">
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import { Plus, RefreshCw } from '@lucide/svelte';
	import Button from '#lib/components/Button.svelte';
	import ProjectPluginSettingsLink from '#lib/components/plugins/ProjectPluginSettingsLink.svelte';
	import { getFormsPageContext } from '#lib/forms/state/context.js';

	const fp = getFormsPageContext();
	const s = fp.state;
	const { loadForms } = fp.loaders;
</script>

<div
	class="flex flex-col gap-3 border-b border-line pb-4 sm:flex-row sm:items-center sm:justify-between"
>
	<div>
		<div class="text-sm text-fg-muted">
			<a href={resolve('/(app)/workspace')} class="hover:text-fg">{$t('forms.workspace')}</a>
			<span class="mx-1 text-fg-muted">/</span>

			<a
				href={resolve('/(app)/workspace/[workspaceId]/projects', { workspaceId: s.workspaceId })}
				class="hover:text-fg">{$t('forms.projects')}</a
			>

			<span class="mx-1 text-fg-muted">/</span>

			<a
				href={resolve('/(app)/workspace/[workspaceId]/projects/[projectId]', {
					workspaceId: s.workspaceId,
					projectId: s.projectId
				})}
				class="hover:text-fg">{s.project?.name ?? $t('forms.projectFallback')}</a
			>
		</div>

		<h1 class="mt-2 text-2xl font-semibold text-fg">
			{$t('forms.title')}
		</h1>
	</div>

	<div class="flex flex-wrap gap-2">
		<ProjectPluginSettingsLink workspaceId={s.workspaceId} projectId={s.projectId} />
		<Button variant="secondary" onclick={() => loadForms()}>
			<RefreshCw class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.refresh')}
		</Button>

		<Button onclick={() => (s.showCreateForm = !s.showCreateForm)}>
			<Plus class="mr-2 h-4 w-4" aria-hidden="true" />
			{$t('forms.newForm')}
		</Button>
	</div>
</div>
