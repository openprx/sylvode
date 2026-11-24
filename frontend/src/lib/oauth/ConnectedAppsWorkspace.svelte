<script lang="ts">
	/**
	 * "Connected applications" in workspace settings (contract 14.3): the switch that lets members
	 * connect applications, and the table of active connections. Shown to owners and admins only:
	 * the API answers 403 or 404 to anyone else, and then nothing is rendered.
	 */
	import { onMount } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { oauthApi, failureKey, isNotEnabled, type ConnectedGrant } from '#lib/api/oauth.js';
	import { plural } from '#lib/ui/foundations/plural.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import Skeleton from '#lib/ui/skeleton/Skeleton.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Switch from '#lib/ui/switch/Switch.svelte';
	import Card from '#lib/components/Card.svelte';
	import ManageNotice from './ManageNotice.svelte';
	import { connectionLabel, relativeTime, scopeSummary } from './grant-summary.js';

	interface Props {
		workspaceId: string;
		workspaceName: string;
	}

	let { workspaceId, workspaceName }: Props = $props();

	let status = $state<'loading' | 'ready' | 'failed' | 'hidden' | 'disabled'>('loading');
	let allow = $state(true);
	let switchValue = $state(true);
	let grants = $state<ConnectedGrant[]>([]);
	let success = $state<string | null>(null);
	let errorKey = $state<string | null>(null);
	let busyId = $state<string | null>(null);
	let switching = $state(false);

	onMount(() => void load());

	async function load() {
		status = 'loading';
		errorKey = null;
		const setting = await oauthApi.getWorkspaceSetting(workspaceId);
		if (!setting.ok) {
			if (isNotEnabled(setting.failure)) {
				status = 'disabled';
				return;
			}
			if (setting.failure.status === 403 || setting.failure.status === 404) {
				status = 'hidden';
				return;
			}
			errorKey = failureKey(setting.failure);
			status = 'failed';
			return;
		}
		allow = setting.data.allow;
		switchValue = allow;
		const listed = await oauthApi.listWorkspace(workspaceId);
		if (!listed.ok) {
			if (isNotEnabled(listed.failure)) {
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

	async function onSwitch(event: Event & { currentTarget: HTMLInputElement }) {
		const wanted = event.currentTarget.checked;
		// The switch shows the saved state until the API confirms the change.
		switchValue = allow;
		if (switching || wanted === allow) return;
		success = null;
		errorKey = null;
		if (!wanted) {
			// The count in the dialog is the API's current count, not the one on screen.
			const fresh = await oauthApi.listWorkspace(workspaceId);
			if (!fresh.ok) {
				errorKey = failureKey(fresh.failure);
				return;
			}
			grants = fresh.data;
			const count = grants.length;
			const confirmed = await confirmAction({
				tone: 'destructive',
				title: $t('oauth.manage.workspace.turnOffTitle', { values: { workspace: workspaceName } }),
				body:
					count === 0
						? $t('oauth.manage.workspace.turnOffBodyNone')
						: plural($t, 'oauth.manage.workspace.turnOffBody', count, $locale),
				confirmLabel:
					count === 0
						? $t('oauth.manage.workspace.turnOffConfirmNone')
						: $t('oauth.manage.workspace.turnOffConfirm')
			});
			if (!confirmed) return;
		}
		switching = true;
		const saved = await oauthApi.setWorkspaceSetting(workspaceId, wanted);
		switching = false;
		if (!saved.ok) {
			errorKey = failureKey(saved.failure);
			return;
		}
		allow = saved.data.allow;
		switchValue = allow;
		if (allow) {
			success = $t('oauth.manage.workspace.turnedOn');
		} else {
			grants = [];
			success =
				saved.data.revokedGrants === 0
					? $t('oauth.manage.workspace.turnedOffNone')
					: plural($t, 'oauth.manage.workspace.turnedOff', saved.data.revokedGrants, $locale);
		}
	}

	async function disconnect(grant: ConnectedGrant) {
		if (busyId !== null) return;
		const client = grant.client.displayName;
		const member = grant.user?.name ?? '';
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: $t('oauth.manage.workspace.dialogTitle', { values: { client, member } }),
			body: $t('oauth.manage.workspace.dialogBody', { values: { client, member } }),
			confirmLabel: $t('oauth.manage.workspace.confirm', { values: { client } })
		});
		if (!confirmed) return;
		busyId = grant.grantId;
		success = null;
		errorKey = null;
		const done = await oauthApi.disconnectInWorkspace(workspaceId, grant.grantId);
		busyId = null;
		if (done.ok) {
			grants = grants.filter((item) => item.grantId !== grant.grantId);
			success = $t('oauth.manage.workspace.done', { values: { client, member } });
			return;
		}
		errorKey = failureKey(done.failure);
		if (done.failure.status === 404)
			grants = grants.filter((item) => item.grantId !== grant.grantId);
	}

	function projectsText(grant: ConnectedGrant): string {
		return grant.projectMode === 'all'
			? $t('oauth.manage.projects.all')
			: plural(
					$t,
					'oauth.manage.projects.selected',
					grant.projectCount ?? grant.projects.length,
					$locale
				);
	}

	function lastUsedText(grant: ConnectedGrant): string {
		if (!grant.lastUsedAt) return $t('oauth.manage.neverUsed');
		return (
			relativeTime(grant.lastUsedAt, new Date(), $locale ?? 'en', $t('oauth.manage.justNow')) ??
			grant.lastUsedAt
		);
	}

	const columns = $derived<Column<ConnectedGrant>[]>([
		{
			id: 'member',
			header: $t('oauth.manage.workspace.member'),
			width: 200,
			frozen: true,
			cell: memberCell
		},
		{
			id: 'application',
			header: $t('oauth.manage.workspace.application'),
			width: 200,
			cell: appCell
		},
		{
			id: 'projects',
			header: $t('oauth.manage.workspace.projectsColumn'),
			width: 140,
			twoLine: 'hidden',
			cell: projectsCell
		},
		{ id: 'access', header: $t('oauth.manage.workspace.access'), width: 150, cell: accessCell },
		{
			id: 'lastUsed',
			header: $t('oauth.manage.workspace.lastUsedColumn'),
			width: 150,
			twoLine: 'hidden',
			cell: usedCell
		}
	]);
</script>

{#snippet memberCell(grant: ConnectedGrant)}{grant.user?.name ?? ''}{/snippet}
{#snippet appCell(grant: ConnectedGrant)}{connectionLabel(
		grant.client.displayName,
		grant.deviceName
	)}{/snippet}
{#snippet projectsCell(grant: ConnectedGrant)}{projectsText(grant)}{/snippet}
{#snippet accessCell(grant: ConnectedGrant)}
	<StatusChip tone="neutral">
		{$t(`oauth.manage.summary.${scopeSummary(grant.scopes)}`)}
	</StatusChip>
{/snippet}
{#snippet usedCell(grant: ConnectedGrant)}{lastUsedText(grant)}{/snippet}
{#snippet rowActions(grant: ConnectedGrant)}
	<Button
		variant="danger-quiet"
		size="sm"
		data-testid="workspace-app-disconnect"
		loading={busyId === grant.grantId}
		loadingLabel={$t('oauth.manage.disconnecting')}
		disabled={busyId !== null}
		onclick={() => void disconnect(grant)}
	>
		{$t('oauth.manage.disconnect')}
	</Button>
{/snippet}

{#if status !== 'hidden'}
	<Card>
		<section
			data-testid="connected-apps-workspace"
			aria-labelledby="connected-apps-workspace-title"
		>
			<h2 id="connected-apps-workspace-title" class="heading">{$t('oauth.manage.title')}</h2>
			<p class="description">{$t('oauth.manage.workspace.description')}</p>

			<ManageNotice
				{success}
				{errorKey}
				testIdPrefix="connected-apps-workspace"
				onretry={status === 'failed' ? () => void load() : undefined}
			/>

			{#if status === 'loading'}
				<div class="skeletons" data-testid="connected-apps-workspace-loading">
					<Skeleton height="32px" label={$t('oauth.manage.loading')} />
					<Skeleton height="48px" />
				</div>
			{:else if status === 'disabled'}
				<StateBlock
					kind="empty"
					message={$t('oauth.notEnabled')}
					data-testid="connected-apps-workspace-disabled"
				/>
			{:else if status === 'ready'}
				<div class="switch-row">
					<Switch
						bind:checked={switchValue}
						disabled={switching}
						label={$t('oauth.manage.workspace.switch')}
						aria-describedby="connected-apps-switch-hint"
						data-testid="connected-apps-allow-switch"
						onchange={onSwitch}
					/>
					<span class="hint" id="connected-apps-switch-hint">
						{$t('oauth.manage.workspace.switchHint')}
					</span>
				</div>

				{#if !allow}
					<p class="empty" data-testid="connected-apps-workspace-off">
						{$t('oauth.manage.workspace.off')}
					</p>
				{:else if grants.length === 0}
					<p class="empty" data-testid="connected-apps-workspace-empty">
						{$t('oauth.manage.workspace.empty')}
					</p>
				{:else}
					<div class="grants-table" data-testid="connected-apps-workspace-table">
						<DataTable
							rows={grants}
							{columns}
							rowId={(grant) => grant.grantId}
							label={$t('oauth.manage.workspace.table')}
							{rowActions}
							rowAttributes={(grant) => ({
								'data-testid': 'workspace-app-row',
								'data-grant-id': grant.grantId
							})}
						/>
					</div>
				{/if}
			{/if}
		</section>
	</Card>
{/if}

<style>
	.heading {
		margin: 0;
		font-size: var(--rf-size-subheading);
		line-height: var(--rf-leading-subheading);
		font-weight: var(--rf-weight-semibold);
		color: var(--rf-text);
	}
	.description,
	.empty,
	.hint {
		font-size: var(--rf-size-secondary);
		line-height: var(--rf-leading-secondary);
		color: var(--rf-secondary);
	}
	.description,
	.empty {
		margin: var(--rf-space-3) 0 var(--rf-space-5);
	}
	.switch-row {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-2);
		margin: var(--rf-space-5) 0;
	}
	.skeletons {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
	.grants-table {
		min-width: 0;
	}
</style>
