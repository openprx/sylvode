<script lang="ts">
	/** "Applications" in instance administration (contract 14.3): the registered client, its status
	 * and the switch that disables it for the whole instance. */
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { oauthApi, failureKey, isNotEnabled, type OAuthClient } from '#lib/api/oauth.js';
	import { plural } from '#lib/ui/foundations/plural.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import Skeleton from '#lib/ui/skeleton/Skeleton.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Card from '#lib/components/Card.svelte';
	import ManageNotice from './ManageNotice.svelte';

	let status = $state<'loading' | 'ready' | 'failed' | 'disabled'>('loading');
	let clients = $state<OAuthClient[]>([]);
	let success = $state<string | null>(null);
	let errorKey = $state<string | null>(null);
	let busyId = $state<string | null>(null);

	onMount(() => void load());

	async function load() {
		status = 'loading';
		errorKey = null;
		const listed = await oauthApi.listClients();
		if (!listed.ok) {
			if (isNotEnabled(listed.failure, true)) {
				status = 'disabled';
				return;
			}
			errorKey = failureKey(listed.failure);
			status = 'failed';
			return;
		}
		clients = listed.data;
		status = 'ready';
	}

	async function change(client: OAuthClient, next: 'active' | 'disabled') {
		if (busyId !== null) return;
		const name = client.displayName;
		if (next === 'disabled') {
			// The count in the dialog is the API's current count, not the one on screen.
			const fresh = await oauthApi.listClients();
			if (!fresh.ok) {
				errorKey = failureKey(fresh.failure);
				return;
			}
			clients = fresh.data;
			const current = clients.find((item) => item.clientId === client.clientId);
			const count = current?.activeGrants ?? 0;
			const confirmed = await confirmAction({
				tone: 'destructive',
				title: $t('oauth.manage.admin.dialogTitle', { values: { client: name } }),
				body:
					count === 0
						? $t('oauth.manage.admin.dialogBodyNone', { values: { client: name } })
						: plural($t, 'oauth.manage.admin.dialogBody', count, $locale, { client: name }),
				confirmLabel: $t('oauth.manage.admin.disable', { values: { client: name } })
			});
			if (!confirmed) return;
		}
		busyId = client.clientId;
		success = null;
		errorKey = null;
		const saved = await oauthApi.setClientStatus(client.clientId, next);
		busyId = null;
		if (!saved.ok) {
			errorKey = failureKey(saved.failure);
			return;
		}
		clients = clients.map((item) =>
			item.clientId === client.clientId
				? {
						...item,
						status: saved.data.status,
						activeGrants: saved.data.status === 'disabled' ? 0 : item.activeGrants
					}
				: item
		);
		success =
			saved.data.status === 'active'
				? $t('oauth.manage.admin.enabledDone', { values: { client: name } })
				: saved.data.revokedGrants === 0
					? $t('oauth.manage.admin.disabledDoneNone', { values: { client: name } })
					: plural($t, 'oauth.manage.admin.disabledDone', saved.data.revokedGrants, $locale, {
							client: name
						});
	}
</script>

<Card>
	<section data-testid="connected-apps-admin" aria-labelledby="connected-apps-admin-title">
		<h2 id="connected-apps-admin-title" class="heading">{$t('oauth.manage.admin.title')}</h2>
		<p class="description">{$t('oauth.manage.admin.description')}</p>

		<ManageNotice
			{success}
			{errorKey}
			testIdPrefix="connected-apps-admin"
			onretry={status === 'failed' ? () => void load() : undefined}
		/>

		{#if status === 'loading'}
			<div class="skeletons" data-testid="connected-apps-admin-loading">
				<Skeleton height="48px" label={$t('oauth.manage.loading')} />
			</div>
		{:else if status === 'disabled'}
			<StateBlock
				kind="empty"
				message={$t('oauth.notEnabled')}
				data-testid="connected-apps-admin-disabled"
			/>
		{:else if status === 'ready'}
			{#if clients.length === 0}
				<p class="empty" data-testid="connected-apps-admin-empty">
					{$t('oauth.manage.admin.empty')}
				</p>
			{:else}
				{#each clients as client (client.clientId)}
					<div data-testid="oauth-client-row" data-client-id={client.clientId}>
						<SettingRow
							label={client.displayName}
							value={plural($t, 'oauth.manage.admin.connections', client.activeGrants, $locale)}
						>
							{#snippet chip()}
								<StatusChip
									tone={client.status === 'active' ? 'success' : 'warning'}
									data-testid="oauth-client-status"
								>
									{client.status === 'active'
										? $t('oauth.manage.admin.active')
										: $t('oauth.manage.admin.disabled')}
								</StatusChip>
							{/snippet}
							{#snippet action()}
								{#if client.status === 'active'}
									<Button
										variant="danger-quiet"
										size="sm"
										data-testid="oauth-client-disable"
										loading={busyId === client.clientId}
										loadingLabel={$t('oauth.manage.disconnecting')}
										disabled={busyId !== null}
										onclick={() => void change(client, 'disabled')}
									>
										{$t('oauth.manage.admin.disable', { values: { client: client.displayName } })}
									</Button>
								{:else}
									<Button
										size="sm"
										data-testid="oauth-client-enable"
										loading={busyId === client.clientId}
										disabled={busyId !== null}
										onclick={() => void change(client, 'active')}
									>
										{$t('oauth.manage.admin.enable', { values: { client: client.displayName } })}
									</Button>
								{/if}
							{/snippet}
						</SettingRow>
					</div>
				{/each}
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
</style>
