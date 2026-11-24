<script lang="ts">
	/** "Connected applications" in personal settings (contract 14.2). */
	import { onMount } from 'svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { locale, t } from 'svelte-i18n';
	import { oauthApi, failureKey, isNotEnabled, type ConnectedGrant } from '#lib/api/oauth.js';
	import { plural } from '#lib/ui/foundations/plural.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import Skeleton from '#lib/ui/skeleton/Skeleton.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Card from '#lib/components/Card.svelte';
	import GrantDetails from './GrantDetails.svelte';
	import ManageNotice from './ManageNotice.svelte';
	import { connectionLabel, relativeTime, scopeSummary } from './grant-summary.js';

	let status = $state<'loading' | 'ready' | 'failed' | 'disabled'>('loading');
	let grants = $state<ConnectedGrant[]>([]);
	let success = $state<string | null>(null);
	let errorKey = $state<string | null>(null);
	let busyId = $state<string | null>(null);
	let open = $state<Set<string>>(new Set());

	onMount(() => void load());

	async function load() {
		status = 'loading';
		errorKey = null;
		const listed = await oauthApi.listOwn();
		if (!listed.ok) {
			if (isNotEnabled(listed.failure, true)) {
				status = 'disabled';
				return;
			}
			errorKey = failureKey(listed.failure);
			status = 'failed';
			return;
		}
		grants = listed.data;
		status = 'ready';
	}

	function valueText(grant: ConnectedGrant): string {
		const projects =
			grant.projectMode === 'all'
				? $t('oauth.manage.projects.all')
				: plural(
						$t,
						'oauth.manage.projects.selected',
						grant.projectCount ?? grant.projects.length,
						$locale
					);
		const used = grant.lastUsedAt
			? $t('oauth.manage.lastUsed', {
					values: {
						time:
							relativeTime(
								grant.lastUsedAt,
								new Date(),
								$locale ?? 'en',
								$t('oauth.manage.justNow')
							) ?? grant.lastUsedAt
					}
				})
			: $t('oauth.manage.neverUsed');
		return $t('oauth.manage.value', {
			values: { workspace: grant.workspace.name, projects, used }
		});
	}

	function toggle(id: string) {
		const next = new SvelteSet(open);
		if (next.has(id)) next.delete(id);
		else next.add(id);
		open = next;
	}

	async function disconnect(grant: ConnectedGrant) {
		if (busyId !== null) return;
		const client = grant.client.displayName;
		const label = connectionLabel(client, grant.deviceName);
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: $t('oauth.manage.personal.dialogTitle', { values: { label } }),
			body: $t('oauth.manage.personal.dialogBody', {
				values: { client, workspace: grant.workspace.name }
			}),
			confirmLabel: $t('oauth.manage.personal.confirm', { values: { client } })
		});
		if (!confirmed) return;
		busyId = grant.grantId;
		success = null;
		errorKey = null;
		const done = await oauthApi.disconnectOwn(grant.grantId);
		busyId = null;
		if (done.ok) {
			grants = grants.filter((item) => item.grantId !== grant.grantId);
			success = $t('oauth.manage.personal.done', { values: { label } });
			return;
		}
		errorKey = failureKey(done.failure);
		// A connection that is already gone leaves the list as well.
		if (done.failure.status === 404)
			grants = grants.filter((item) => item.grantId !== grant.grantId);
	}
</script>

<Card>
	<section data-testid="connected-apps-personal" aria-labelledby="connected-apps-personal-title">
		<h2 id="connected-apps-personal-title" class="heading">{$t('oauth.manage.title')}</h2>
		<p class="description">{$t('oauth.manage.personal.description')}</p>

		<ManageNotice
			{success}
			{errorKey}
			testIdPrefix="connected-apps-personal"
			onretry={status === 'failed' ? () => void load() : undefined}
		/>

		{#if status === 'loading'}
			<div class="skeletons" data-testid="connected-apps-personal-loading">
				<Skeleton height="48px" label={$t('oauth.manage.loading')} />
				<Skeleton height="48px" />
			</div>
		{:else if status === 'disabled'}
			<StateBlock
				kind="empty"
				message={$t('oauth.notEnabled')}
				data-testid="connected-apps-personal-disabled"
			/>
		{:else if status === 'ready'}
			{#if grants.length === 0}
				<p class="empty" data-testid="connected-apps-personal-empty">
					{$t('oauth.manage.personal.empty')}
				</p>
			{:else}
				<div data-testid="connected-apps-personal-list">
					{#each grants as grant (grant.grantId)}
						{@const label = connectionLabel(grant.client.displayName, grant.deviceName)}
						<div data-testid="connected-app-row" data-grant-id={grant.grantId}>
							<SettingRow {label} value={valueText(grant)}>
								{#snippet chip()}
									<StatusChip tone="neutral">
										{$t(`oauth.manage.summary.${scopeSummary(grant.scopes)}`)}
									</StatusChip>
								{/snippet}
								{#snippet action()}
									<span class="actions">
										<Button
											variant="text"
											size="sm"
											aria-expanded={open.has(grant.grantId)}
											aria-controls={`grant-details-${grant.grantId}`}
											aria-label={$t('oauth.manage.details.toggle', { values: { label } })}
											data-testid="connected-app-details"
											onclick={() => toggle(grant.grantId)}
										>
											{$t('oauth.manage.details.button')}
										</Button>
										<Button
											variant="danger-quiet"
											size="sm"
											data-testid="connected-app-disconnect"
											loading={busyId === grant.grantId}
											loadingLabel={$t('oauth.manage.disconnecting')}
											disabled={busyId !== null}
											onclick={() => void disconnect(grant)}
										>
											{$t('oauth.manage.disconnect')}
										</Button>
									</span>
								{/snippet}
							</SettingRow>
							{#if open.has(grant.grantId)}
								<GrantDetails {grant} id={`grant-details-${grant.grantId}`} />
							{/if}
						</div>
					{/each}
				</div>
			{/if}
		{/if}
	</section>
</Card>

<style>
	.heading {
		margin: 0;
		font-size: var(--rf-size-subheading);
		line-height: var(--rf-leading-subheading);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.description,
	.empty {
		margin: var(--rf-space-3) 0 var(--rf-space-5);
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.skeletons {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.actions {
		display: inline-flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
		justify-content: flex-end;
	}
</style>
