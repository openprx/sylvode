<script lang="ts">
	/** The expanded part of a connection row: access, projects, connection and expiry dates. */
	import { locale, t } from 'svelte-i18n';
	import type { ConnectedGrant } from '#lib/api/oauth.js';
	import { formatDate, scopeText } from './grant-summary.js';

	interface Props {
		grant: ConnectedGrant;
		id: string;
	}

	let { grant, id }: Props = $props();

	const shown = $derived(grant.projects.length);
	const total = $derived(grant.projectCount ?? shown);
</script>

<dl class="details" {id} data-testid="connected-app-details-panel">
	<div>
		<dt>{$t('oauth.manage.details.access')}</dt>
		<dd>
			<ul>
				{#each grant.scopes as scope (scope)}
					<li>{scopeText($t, scope, grant.client.displayName)}</li>
				{/each}
			</ul>
		</dd>
	</div>
	<div>
		<dt>{$t('oauth.manage.details.projects')}</dt>
		<dd>
			{#if grant.projectMode === 'all'}
				{$t('oauth.manage.details.allProjects')}
			{:else}
				<ul>
					{#each grant.projects as project (project.id)}
						<li>{project.name} <span class="muted">{project.key}</span></li>
					{/each}
					{#if total > shown}
						<li class="muted">
							{$t('oauth.manage.details.moreProjects', { values: { count: total - shown } })}
						</li>
					{/if}
				</ul>
			{/if}
		</dd>
	</div>
	<div>
		<dt>{$t('oauth.manage.details.connectedOn')}</dt>
		<dd>{formatDate(grant.consentedAt, $locale ?? 'en')}</dd>
	</div>
	<div>
		<dt>{$t('oauth.manage.details.expires')}</dt>
		<dd>{formatDate(grant.expiresAt, $locale ?? 'en')}</dd>
	</div>
</dl>

<style>
	.details {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
		margin: 0;
		padding: var(--rf-space-5) 0 var(--rf-space-5) var(--rf-space-5);
		border-bottom: var(--rf-border-width) solid var(--rf-line);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
	}
	dt {
		color: var(--rf-secondary);
	}
	dd {
		margin: 0;
		color: var(--rf-text);
		overflow-wrap: anywhere;
	}
	ul {
		margin: 0;
		padding-inline-start: var(--rf-space-6);
	}
	.muted {
		color: var(--rf-secondary);
	}
</style>
