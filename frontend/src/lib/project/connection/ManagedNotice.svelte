<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { executionHref, type ManagedBy } from './managed';

	interface Props {
		workspaceId: string;
		managedBy: ManagedBy;
	}
	let { workspaceId, managedBy }: Props = $props();
</script>

<span class="notice" data-testid="managed-notice">
	<StatusChip tone="neutral">{$t('execution.connection.managed.chip')}</StatusChip>
	{#each managedBy.projects as project (project.id)}
		<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- full path built by executionHref -->
		<a class="project" href={executionHref(workspaceId, project.id)}>{project.name}</a>
	{/each}
</span>

<style>
	.notice {
		display: inline-flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-2) var(--rf-space-4);
	}
	.project {
		color: var(--rf-text);
		font-size: var(--rf-size-meta);
		text-decoration: underline;
	}
</style>
