<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import Trash2 from '@lucide/svelte/icons/trash-2';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Avatar from '#lib/ui/avatar/Avatar.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import DataTable from '#lib/ui/data-table/DataTable.svelte';
	import type { Column } from '#lib/ui/data-table/data-table.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import IconButton from '#lib/ui/icon-button/IconButton.svelte';
	import Radio from '#lib/ui/radio/Radio.svelte';
	import RadioGroup from '#lib/ui/radio/RadioGroup.svelte';
	import Select from '#lib/ui/select/Select.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import { sitesApi } from './api';
	import { errorText } from './errors';
	import { roleCandidates } from './view';
	import type { SiteRole } from './wire';

	/**
	 * Who can sign in on the site: operators work on records, editors also change pages and the
	 * title. Everybody reads the list; only owners and admins add, change or remove a role.
	 */
	let {
		siteId,
		workspaceId,
		isAdmin,
		onchanged
	}: {
		siteId: string;
		workspaceId: string;
		isAdmin: boolean;
		/** The list changed, so the page header can re-read the viewer's own role. */
		onchanged: () => void;
	} = $props();

	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let problem = $state('');
	let roles = $state.raw<SiteRole[]>([]);
	let members = $state.raw<WorkspaceMember[]>([]);
	let busy = $state(false);
	let formError = $state('');
	let newUser = $state<string | null>(null);
	let newRole = $state<SiteRole['role']>('site_operator');

	const byId = $derived(new Map(members.map((member) => [member.user_id, member])));
	const candidates = $derived(roleCandidates(members, roles));
	const roleOptions = $derived(
		(['site_operator', 'site_editor'] as const).map((value) => ({
			value,
			label: $t(`sites.roles.role_${value}`)
		}))
	);

	async function load(): Promise<void> {
		phase = 'loading';
		const [rolesResult, membersResult] = await Promise.all([
			sitesApi.roles(siteId),
			membersApi.list(workspaceId)
		]);
		if (rolesResult.code !== 0 || !rolesResult.data) {
			problem = describeResult(rolesResult);
			phase = 'failed';
			return;
		}
		roles = rolesResult.data.items;
		members = membersResult.data?.items ?? [];
		phase = 'ready';
	}

	onMount(() => {
		void load();
	});

	const nameOf = (userId: string): string => byId.get(userId)?.name || userId.slice(0, 8);

	async function add(): Promise<void> {
		if (!newUser) return;
		formError = '';
		busy = true;
		const result = await sitesApi.putRole(siteId, newUser, newRole);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.roles.added'));
			newUser = null;
			await load();
			onchanged();
		} else formError = errorText(result, $t);
	}

	async function change(role: SiteRole, next: SiteRole['role']): Promise<void> {
		if (next === role.role) return;
		busy = true;
		const result = await sitesApi.putRole(siteId, role.user_id, next);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.roles.changed'));
			await load();
			onchanged();
		} else toast.error(errorText(result, $t));
	}

	async function remove(role: SiteRole): Promise<void> {
		const yes = await confirmAction({
			tone: 'destructive',
			title: $t('sites.roles.removeTitle', { values: { name: nameOf(role.user_id) } }),
			body: $t('sites.roles.removeBody'),
			confirmLabel: $t('sites.roles.removeConfirm')
		});
		if (!yes) return;
		busy = true;
		const result = await sitesApi.removeRole(siteId, role.user_id);
		busy = false;
		if (result.code === 0) {
			toast.success($t('sites.roles.removed'));
			await load();
			onchanged();
		} else toast.error(errorText(result, $t));
	}

	const columns = $derived<Column<SiteRole>[]>([
		{ id: 'member', header: $t('sites.roles.col.member'), width: 320, cell: memberCell },
		{ id: 'role', header: $t('sites.roles.col.role'), width: 240, cell: roleCell },
		...(isAdmin
			? [{ id: 'actions', header: '', width: 80, align: 'end' as const, cell: actionsCell }]
			: [])
	]);
</script>

{#snippet memberCell(role: SiteRole)}
	<span class="person">
		<Avatar name={nameOf(role.user_id)} size="sm" decorative />
		<span>
			<span class="name">{nameOf(role.user_id)}</span>
			{#if byId.get(role.user_id)?.email}
				<span class="muted">{byId.get(role.user_id)?.email}</span>
			{/if}
		</span>
	</span>
{/snippet}
{#snippet roleCell(role: SiteRole)}
	{#if isAdmin}
		<Select
			label={$t('sites.roles.col.role')}
			labelHidden
			value={role.role}
			options={roleOptions}
			disabled={busy}
			onchange={(value) => value && void change(role, value)}
		/>
	{:else}
		<StatusChip>{$t(`sites.roles.role_${role.role}`)}</StatusChip>
	{/if}
{/snippet}
{#snippet actionsCell(role: SiteRole)}
	<IconButton
		icon={Trash2}
		label={$t('sites.roles.remove')}
		disabled={busy}
		onclick={() => void remove(role)}
		data-testid="remove-role"
	/>
{/snippet}

<div class="roles" data-testid="site-roles">
	<p class="lead">{isAdmin ? $t('sites.roles.lead') : $t('sites.roles.readOnlyLead')}</p>

	{#if isAdmin && phase === 'ready'}
		<form
			class="add"
			onsubmit={(event) => {
				event.preventDefault();
				void add();
			}}
		>
			<Select
				label={$t('sites.roles.person')}
				value={newUser}
				options={candidates.map((member) => ({
					value: member.user_id,
					label: member.name,
					meta: member.email
				}))}
				placeholder={$t('sites.roles.personPlaceholder')}
				helper={candidates.length === 0 ? $t('sites.roles.noCandidates') : undefined}
				disabled={candidates.length === 0}
				searchable="auto"
				onchange={(value) => (newUser = value)}
				testId="role-person"
			/>
			<RadioGroup label={$t('sites.roles.roleLabel')} bind:value={newRole}>
				<Radio value="site_operator" label={$t('sites.roles.operatorText')} />
				<Radio value="site_editor" label={$t('sites.roles.editorText')} />
			</RadioGroup>
			<div>
				<Button
					variant="primary"
					type="submit"
					loading={busy}
					disabled={!newUser}
					data-testid="add-role"
				>
					{$t('sites.roles.add')}
				</Button>
			</div>
			{#if formError}<TipBar tone="danger">{formError}</TipBar>{/if}
		</form>
	{/if}

	{#if phase === 'loading'}
		<StateBlock kind="loading" thing={$t('sites.roles.thing')} />
	{:else if phase === 'failed'}
		<StateBlock
			kind="loadFailed"
			thing={$t('sites.roles.thing')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else if roles.length === 0}
		<StateBlock kind="empty" thing={$t('sites.roles.thing')} message={$t('sites.roles.empty')} />
	{:else}
		<DataTable
			rows={roles}
			{columns}
			rowId={(role) => role.user_id}
			rowLabel={(role) => nameOf(role.user_id)}
			label={$t('sites.roles.title')}
			thing={$t('sites.roles.thing')}
		/>
	{/if}
</div>

<style>
	.roles {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.lead {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.add {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 480px;
	}
	.person {
		display: inline-flex;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.name {
		display: block;
	}
	.muted {
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
