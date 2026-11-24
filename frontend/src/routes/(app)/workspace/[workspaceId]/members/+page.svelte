<script lang="ts">
	import { agentTypeKey } from '#lib/admin/agent-types.js';
	import BotAccessDialog from '#lib/admin/bot-access/BotAccessDialog.svelte';
	import { accessSummary } from '#lib/admin/bot-access/rules.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import { page } from '$app/state';
	import { botsApi, type Bot, type CreateBotResponse } from '#lib/api/bots.js';
	import {
		BOT_TRANSPORT_SURFACES,
		isBotTransportSurface,
		surfaceDescriptionKey,
		surfaceLabelKey,
		type BotTransportSurface
	} from '#lib/bots/transport-surfaces.js';
	import { createBotRequest } from '#lib/bots/create-bot-request.js';
	import { membersApi, type WorkspaceMember, type WorkspaceMemberRole } from '#lib/api/members.js';
	import { copyText } from '#lib/clipboard.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import Button from '#lib/ui/button/Button.svelte';
	import BotIcon from '#lib/components/BotIcon.svelte';
	import Input from '#lib/components/Input.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import ManagedNotice from '#lib/project/connection/ManagedNotice.svelte';
	import { managedBotIds, managedLock, parseManagedBy } from '#lib/project/connection/managed.js';

	type EditableRole = Exclude<WorkspaceMemberRole, 'owner'>;
	type BotPermission = 'read' | 'write' | 'admin';

	interface SearchUser {
		id: string;
		email: string;
		name: string;
	}

	const workspaceId = requireRouteParam(page.params.workspaceId, 'workspaceId');
	const botPermissionOptions: BotPermission[] = ['read', 'write', 'admin'];

	let loading = $state(true);
	let removingMemberId = $state<string | null>(null);
	let changingRoleMemberId = $state<string | null>(null);
	let addingUserId = $state<string | null>(null);
	let searchingUsers = $state(false);

	let members = $state<WorkspaceMember[]>([]);
	let showInviteModal = $state(false);
	let selectedRole = $state<EditableRole>('member');
	let searchKeyword = $state('');
	let searchResults = $state<SearchUser[]>([]);

	let botsLoading = $state(true);
	let creatingToken = $state(false);
	let revokingBotId = $state<string | null>(null);
	let showCreateTokenModal = $state(false);
	let showCreatedTokenModal = $state(false);
	let tokenName = $state('');
	let tokenExpiresAt = $state('');
	let tokenPermissions = $state<BotPermission[]>(['read']);
	// Deliberately no default: a token works on exactly one surface, and silently picking one
	// is how every token created here used to end up usable for the REST API only.
	let tokenSurface = $state<BotTransportSurface | ''>('');
	let bots = $state<Bot[]>([]);
	let createdToken = $state<CreateBotResponse | null>(null);
	let showAccessDialog = $state(false);
	let accessBot = $state<Bot | null>(null);

	/** Bots a project connection manages: their token and seat are changed on the project. */
	const managedBots = $derived(managedBotIds(bots));
	/** A managed bot's member row cannot be removed here: it leaves with its last connection. */
	const memberLock = (userId: string) =>
		managedLock(managedBots.has(userId) ? { kind: 'project_connection', projects: [] } : null);
	const memberIds = $derived(new Set(members.map((member) => member.user_id)));

	onMount(async () => {
		await Promise.all([loadMembers(), loadBots()]);
	});

	async function loadMembers() {
		loading = true;
		const response = await membersApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(describeResult(response));
		} else if (response.data) {
			members = response.data.items ?? [];
		}
		loading = false;
	}

	async function loadBots() {
		botsLoading = true;
		const response = await botsApi.list(workspaceId);
		if (response.code !== 0) {
			toast.error(describeResult(response));
			bots = [];
		} else {
			bots = response.data ?? [];
		}
		botsLoading = false;
	}

	async function searchInternalUsers() {
		searchingUsers = true;
		const response = await membersApi.searchUsers(workspaceId, {
			search: searchKeyword.trim(),
			limit: 20
		});
		if (response.code !== 0) {
			toast.error(describeResult(response));
			searchResults = [];
		} else {
			searchResults = response.data?.items ?? [];
		}
		searchingUsers = false;
	}

	async function addInternalUser(userId: string) {
		addingUserId = userId;
		const response = await membersApi.add(workspaceId, {
			user_id: userId,
			role: selectedRole
		});

		if (response.code !== 0) {
			toast.error(describeResult(response));
		} else {
			toast.success(get(t)('members.addSuccess'));
			await loadMembers();
		}

		addingUserId = null;
	}

	async function removeMember(member: WorkspaceMember) {
		const name = member.name || member.email || member.user_id;
		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('members.removeTitle', { values: { name } }),
				body: get(t)('members.removeBody', { values: { name } }),
				confirmLabel: get(t)('members.removeAction', { values: { name } })
			}))
		) {
			return;
		}

		removingMemberId = member.user_id;
		const response = await membersApi.remove(workspaceId, member.user_id);
		if (response.code !== 0) {
			toast.error(describeResult(response));
		} else {
			toast.success(get(t)('members.removeSuccess'));
			await loadMembers();
		}
		removingMemberId = null;
	}

	async function changeRole(member: WorkspaceMember, nextRole: WorkspaceMemberRole) {
		if (member.role === nextRole || nextRole === 'owner') {
			return;
		}

		const roleValues = { values: { email: member.email, role: nextRole } };
		if (
			!(await confirmAction({
				tone: 'neutral',
				title: get(t)('members.changeRoleTitle', roleValues),
				body: get(t)('members.changeRoleBody', roleValues),
				confirmLabel: get(t)('members.changeRoleAction', roleValues)
			}))
		) {
			return;
		}

		changingRoleMemberId = member.user_id;
		const response = await membersApi.updateRole(workspaceId, member.user_id, nextRole);

		if (response.code !== 0) {
			toast.error(get(t)('members.changeRoleFailed', { values: { message: response.message } }));
		} else {
			toast.success(get(t)('members.changeRoleSuccess'));
			await loadMembers();
		}
		changingRoleMemberId = null;
	}

	function formatDate(dateText: string | null | undefined): string {
		if (!dateText) return '-';
		const parsed = new Date(dateText);
		if (Number.isNaN(parsed.getTime())) return '-';
		return parsed.toLocaleString(get(locale) === 'en' ? 'en-US' : 'zh-CN', {
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit'
		});
	}

	function isBotMember(member: WorkspaceMember): boolean {
		return member.entity_type === 'bot';
	}

	function getBotRoleLabel(member: WorkspaceMember): string {
		const botLabel = get(t)('admin.botBadge');
		const key = agentTypeKey(member.agent_type);
		return key ? `${botLabel} (${get(t)(key)})` : botLabel;
	}

	function openCreateTokenModal() {
		tokenName = '';
		tokenExpiresAt = '';
		tokenPermissions = ['read'];
		tokenSurface = '';
		showCreateTokenModal = true;
	}

	function togglePermission(permission: BotPermission) {
		if (tokenPermissions.includes(permission)) {
			if (tokenPermissions.length === 1) return;
			tokenPermissions = tokenPermissions.filter((value) => value !== permission);
			return;
		}
		tokenPermissions = [...tokenPermissions, permission];
	}

	function buildExpiresAt(dateInput: string): string | undefined {
		if (!dateInput) return undefined;
		const parsed = new Date(`${dateInput}T23:59:59`);
		if (Number.isNaN(parsed.getTime())) return undefined;
		return parsed.toISOString();
	}

	async function createToken() {
		const built = createBotRequest({
			name: tokenName,
			permissions: tokenPermissions,
			surface: tokenSurface,
			expiresAt: buildExpiresAt(tokenExpiresAt)
		});
		if (!built.ok) {
			toast.error(get(t)(built.errorKey));
			return;
		}

		creatingToken = true;
		const response = await botsApi.create(workspaceId, built.request);

		if (response.code !== 0) {
			toast.error(describeResult(response));
		} else if (response.data) {
			createdToken = response.data;
			showCreateTokenModal = false;
			showCreatedTokenModal = true;
			toast.success(get(t)('members.createTokenSuccess'));
			await loadBots();
		}

		creatingToken = false;
	}

	function openAccess(bot: Bot) {
		accessBot = bot;
		showAccessDialog = true;
	}

	async function revokeToken(bot: Bot) {
		const nameValues = { values: { name: bot.name } };
		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('members.revokeTitle', nameValues),
				body: get(t)('members.revokeBody'),
				confirmLabel: get(t)('members.revokeAction', nameValues)
			}))
		) {
			return;
		}

		revokingBotId = bot.id;
		const response = await botsApi.revoke(workspaceId, bot.id);
		if (response.code !== 0) {
			toast.error(describeResult(response));
		} else {
			toast.success(get(t)('members.revokeSuccess'));
			await loadBots();
		}
		revokingBotId = null;
	}

	async function copyCreatedToken() {
		if (!createdToken?.token) return;
		if (await copyText(createdToken.token)) toast.success(get(t)('members.copyTokenSuccess'));
		else toast.error(get(t)('members.copyTokenFailed'));
	}

	function closeCreatedTokenModal() {
		showCreatedTokenModal = false;
		createdToken = null;
	}

	function getPermissionClass(permission: string): string {
		if (permission === 'admin') {
			return 'bg-danger-soft text-danger';
		}
		if (permission === 'write') {
			return 'bg-warning-soft text-warning';
		}
		return 'bg-surface text-fg';
	}

	/** A surface the server reports but this client does not know is shown verbatim. */
	function getSurfaceLabel(surface: string): string {
		return isBotTransportSurface(surface) ? get(t)(surfaceLabelKey(surface)) : surface;
	}

	function getSurfaceDescription(surface: string): string {
		return isBotTransportSurface(surface) ? get(t)(surfaceDescriptionKey(surface)) : '';
	}

	function getPermissionLabel(permission: string): string {
		const normalized = permission.toLowerCase();
		if (normalized === 'admin' || normalized === 'write' || normalized === 'read') {
			return get(t)(`members.permission.${normalized}`);
		}
		return permission;
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div class="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
		<div>
			<h1 class="text-2xl font-semibold text-fg">{$t('members.title')}</h1>
			<p class="mt-1 text-sm text-fg-muted">{$t('members.subtitle')}</p>
		</div>

		<Button variant="primary" onclick={() => (showInviteModal = true)}
			>{$t('members.addInternalUser')}</Button
		>
	</div>

	<div class="rounded-card border border-line bg-surface p-4 text-sm text-fg">
		{$t('members.roleDescription')}
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('common.loading')}
		</div>
	{:else if members.length === 0}
		<div class="rounded-card border border-line bg-raised p-10 text-center text-fg-muted">
			{$t('members.empty')}
		</div>
	{:else}
		<div class="overflow-hidden rounded-card border border-line bg-raised">
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-line">
					<thead class="bg-nav">
						<tr>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.member')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('admin.email')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('admin.role')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.joinedAt')}</th
							>
							<th
								class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.actions')}</th
							>
						</tr>
					</thead>
					<tbody class="divide-y divide-line bg-raised">
						{#each members as member (member.user_id)}
							<tr>
								<td class="px-4 py-3 text-sm font-medium text-fg">
									<div class="flex items-center gap-1.5">
										{#if isBotMember(member)}
											<BotIcon class="h-4 w-4 text-fg-muted" title={$t('admin.botBadge')} />
										{/if}
										<span>{member.name || member.user_id}</span>
									</div>
								</td>
								<td class="px-4 py-3 text-sm text-fg-muted">{member.email || '-'}</td>
								<td class="px-4 py-3 text-sm text-fg-muted">
									{#if isBotMember(member)}
										<span
											class="inline-flex rounded-full bg-surface px-2.5 py-1 text-xs font-medium text-fg"
										>
											{getBotRoleLabel(member)}
										</span>
									{:else}
										<select
											value={member.role}
											disabled={member.role === 'owner' || changingRoleMemberId === member.user_id}
											onchange={(event) => {
												const target = event.currentTarget as HTMLSelectElement;
												changeRole(member, target.value as WorkspaceMemberRole);
											}}
											class="rounded-control border border-input bg-field px-2 py-1 text-sm text-fg disabled:cursor-not-allowed disabled:bg-surface"
										>
											<option value="owner">{$t('roles.owner')}</option>
											<option value="admin">{$t('roles.admin')}</option>
											<option value="member">{$t('roles.member')}</option>
										</select>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-fg-muted">{formatDate(member.joined_at)}</td>
								<td class="px-4 py-3 text-right">
									<Button
										variant="danger-quiet"
										size="sm"
										disabled={member.role === 'owner' || memberLock(member.user_id).disabled}
										disabledReason={memberLock(member.user_id).reasonKey
											? $t(memberLock(member.user_id).reasonKey ?? '')
											: undefined}
										loading={removingMemberId === member.user_id}
										onclick={() => removeMember(member)}
									>
										{$t('members.remove')}
									</Button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</div>
	{/if}

	<div class="overflow-hidden rounded-card border border-line bg-raised">
		<div
			class="flex flex-col gap-3 border-b border-line p-4 sm:flex-row sm:items-center sm:justify-between"
		>
			<div>
				<h2 class="text-lg font-semibold text-fg">
					{$t('members.apiTokensTitle')}
				</h2>
				<p class="mt-1 text-sm text-fg-muted">
					{$t('members.apiTokensSubtitle')}
				</p>
			</div>
			<Button variant="primary" size="sm" onclick={openCreateTokenModal}
				>{$t('members.createToken')}</Button
			>
		</div>

		{#if botsLoading}
			<div class="p-6 text-sm text-fg-muted">{$t('common.loading')}</div>
		{:else if bots.length === 0}
			<div class="p-10 text-center text-sm text-fg-muted">
				{$t('members.apiTokensEmpty')}
			</div>
		{:else}
			<div class="overflow-x-auto">
				<table class="min-w-full divide-y divide-line">
					<thead class="bg-nav">
						<tr>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.tokenName')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.tokenPrefix')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.transportSurfaceColumn')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.permissions')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('botAccess.column')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.status')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('members.lastUsedAt')}</th
							>
							<th
								class="px-4 py-3 text-left text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.createdAt')}</th
							>
							<th
								class="px-4 py-3 text-right text-xs font-medium uppercase tracking-wider text-fg-muted"
								>{$t('common.actions')}</th
							>
						</tr>
					</thead>
					<tbody class="divide-y divide-line bg-raised">
						{#each bots as bot (bot.id)}
							<tr>
								<td class="px-4 py-3 text-sm font-medium text-fg">
									<div>{bot.name}</div>
									{#if parseManagedBy(bot.managed_by)}
										{@const managed = parseManagedBy(bot.managed_by)}
										{#if managed}
											<div class="mt-1"><ManagedNotice {workspaceId} managedBy={managed} /></div>
										{/if}
									{/if}
								</td>
								<td class="px-4 py-3 text-sm font-mono text-fg-muted">{bot.token_prefix}</td>
								<td class="px-4 py-3 text-sm text-fg-muted">
									<span
										class="inline-flex rounded-full bg-surface px-2.5 py-1 text-xs font-medium text-fg"
										title={getSurfaceDescription(bot.transport_surface)}
										data-testid="bot-transport-surface"
										data-surface={bot.transport_surface}
									>
										{getSurfaceLabel(bot.transport_surface)}
									</span>
								</td>
								<td class="px-4 py-3 text-sm text-fg-muted">
									<div class="flex flex-wrap gap-1.5">
										{#if bot.permissions.length === 0}
											<span>-</span>
										{:else}
											{#each bot.permissions as permission, index (index)}
												<span
													class={`inline-flex rounded-full px-2.5 py-1 text-xs font-medium ${getPermissionClass(permission)}`}
												>
													{getPermissionLabel(permission)}
												</span>
											{/each}
										{/if}
									</div>
								</td>
								<td
									class="px-4 py-3 text-sm text-fg-muted"
									data-testid="bot-project-scope"
									data-scope={bot.project_scope ?? 'workspace'}
								>
									{accessSummary(bot, managedBots.has(bot.id), $t, $locale)}
								</td>
								<td class="px-4 py-3 text-sm">
									{#if bot.is_active}
										<span
											class="inline-flex rounded-full bg-success-soft px-2.5 py-1 text-xs font-medium text-success"
										>
											{$t('members.tokenStatus.active')}
										</span>
									{:else}
										<span
											class="inline-flex rounded-full bg-surface px-2.5 py-1 text-xs font-medium text-fg"
										>
											{$t('members.tokenStatus.disabled')}
										</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-sm text-fg-muted">{formatDate(bot.last_used_at)}</td>
								<td class="px-4 py-3 text-sm text-fg-muted">{formatDate(bot.created_at)}</td>
								<td class="px-4 py-3 text-right">
									<Button
										variant="secondary"
										size="sm"
										disabled={managedLock(parseManagedBy(bot.managed_by)).disabled}
										disabledReason={managedLock(parseManagedBy(bot.managed_by)).reasonKey
											? $t(managedLock(parseManagedBy(bot.managed_by)).reasonKey ?? '')
											: undefined}
										data-testid="bot-access-open"
										onclick={() => openAccess(bot)}
									>
										{$t('botAccess.button')}
									</Button>
									<Button
										variant="danger-quiet"
										size="sm"
										disabled={!bot.is_active ||
											managedLock(parseManagedBy(bot.managed_by)).disabled}
										disabledReason={managedLock(parseManagedBy(bot.managed_by)).reasonKey
											? $t(managedLock(parseManagedBy(bot.managed_by)).reasonKey ?? '')
											: undefined}
										loading={revokingBotId === bot.id}
										onclick={() => revokeToken(bot)}
									>
										{$t('members.revoke')}
									</Button>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={showInviteModal} title={$t('members.addInternalUser')}>
	<div class="space-y-4">
		<form
			onsubmit={(event) => {
				event.preventDefault();
				searchInternalUsers();
			}}
			class="grid grid-cols-1 gap-3 sm:grid-cols-[1fr_auto]"
		>
			<Input
				type="search"
				label={$t('members.usernameOrEmail')}
				bind:value={searchKeyword}
				placeholder={$t('members.usernameOrEmailPlaceholder')}
			/>
			<div class="flex items-end">
				<Button variant="primary" type="submit" loading={searchingUsers}
					>{$t('common.search')}</Button
				>
			</div>
		</form>

		<div>
			<label class="mb-1 block text-sm font-medium text-fg" for="inviteRole"
				>{$t('admin.role')}</label
			>
			<select
				id="inviteRole"
				bind:value={selectedRole}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			>
				<option value="member">{$t('roles.member')}</option>
				<option value="admin">{$t('roles.admin')}</option>
			</select>
		</div>

		<div class="max-h-72 space-y-2 overflow-y-auto rounded-control border border-line p-2">
			{#if searchingUsers}
				<p class="p-2 text-sm text-fg-muted">{$t('search.searching')}</p>
			{:else if searchResults.length === 0}
				<p class="p-2 text-sm text-fg-muted">
					{$t('members.noSearchResults')}
				</p>
			{:else}
				{#each searchResults as user (user.id)}
					<div class="flex items-center justify-between rounded-row px-2 py-2 hover:bg-surface">
						<div>
							<p class="text-sm font-medium text-fg">{user.name}</p>
							<p class="text-xs text-fg-muted">{user.email}</p>
						</div>
						{#if memberIds.has(user.id)}
							<Button size="sm" variant="secondary" disabled>{$t('members.added')}</Button>
						{:else}
							<Button
								variant="primary"
								size="sm"
								loading={addingUserId === user.id}
								onclick={() => addInternalUser(user.id)}
							>
								{$t('common.create')}
							</Button>
						{/if}
					</div>
				{/each}
			{/if}
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showInviteModal = false)}
				>{$t('common.close')}</Button
			>
		</div>
	</div>
</Modal>

<Modal bind:open={showCreateTokenModal} title={$t('members.createToken')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			createToken();
		}}
		class="space-y-4"
	>
		<Input
			label={$t('members.tokenName')}
			placeholder={$t('members.tokenNamePlaceholder')}
			bind:value={tokenName}
			required
		/>

		<fieldset aria-describedby="tokenSurfaceHint">
			<legend class="mb-1 block text-sm font-medium text-fg">
				{$t('members.transportSurface')}
				<span class="text-danger" aria-hidden="true">*</span>
			</legend>
			<p id="tokenSurfaceHint" class="mb-2 text-xs text-fg-muted">
				{$t('members.transportSurfaceHint')}
			</p>
			<div class="space-y-2">
				{#each BOT_TRANSPORT_SURFACES as surface (surface)}
					<label
						class="flex cursor-pointer items-start gap-3 rounded-control border px-3 py-2 text-sm {tokenSurface ===
						surface
							? 'border-fg bg-surface'
							: 'border-input bg-field'}"
					>
						<input
							type="radio"
							name="tokenSurface"
							value={surface}
							bind:group={tokenSurface}
							required
							aria-describedby={`tokenSurface-${surface}-description`}
							class="mt-0.5 h-4 w-4 border-input accent-primary"
						/>
						<span>
							<span class="block font-medium text-fg">{$t(surfaceLabelKey(surface))}</span>
							<span id={`tokenSurface-${surface}-description`} class="block text-xs text-fg-muted"
								>{$t(surfaceDescriptionKey(surface))}</span
							>
						</span>
					</label>
				{/each}
			</div>
		</fieldset>

		<div>
			<p class="mb-2 block text-sm font-medium text-fg">
				{$t('members.permissions')}
			</p>
			<div class="flex flex-wrap gap-2">
				{#each botPermissionOptions as permission, index (index)}
					<label
						class="inline-flex items-center gap-2 rounded-control border border-input bg-field px-3 py-1.5 text-sm text-fg"
					>
						<input
							type="checkbox"
							checked={tokenPermissions.includes(permission)}
							onchange={() => togglePermission(permission)}
							class="h-4 w-4 rounded border-input accent-primary"
						/>
						<span>{$t(`members.permission.${permission}`)}</span>
					</label>
				{/each}
			</div>
		</div>

		<div>
			<label class="mb-1 block text-sm font-medium text-fg" for="tokenExpiresAt"
				>{$t('members.expiresAt')}</label
			>
			<input
				id="tokenExpiresAt"
				type="date"
				bind:value={tokenExpiresAt}
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			/>
			<p class="mt-1 text-xs text-fg-muted">{$t('members.expiresAtHint')}</p>
		</div>

		<div class="flex justify-end gap-2 pt-2">
			<Button variant="secondary" onclick={() => (showCreateTokenModal = false)}
				>{$t('common.cancel')}</Button
			>

			<Button variant="primary" type="submit" loading={creatingToken}
				>{$t('members.createToken')}</Button
			>
		</div>
	</form>
</Modal>

<Modal
	bind:open={showCreatedTokenModal}
	title={$t('members.tokenCreatedTitle')}
	maxWidthClass="max-w-2xl"
	onclose={closeCreatedTokenModal}
>
	{#if createdToken}
		<div class="space-y-4">
			<div class="rounded-control border bg-warning-soft p-3 text-sm text-warning">
				{$t('members.tokenCreatedHint')}
			</div>

			<div
				class="rounded-control border bg-surface p-3 text-sm text-fg"
				data-testid="created-token-surface"
			>
				<p class="font-medium">
					{$t('members.tokenWorksOn', {
						values: { surface: getSurfaceLabel(createdToken.transport_surface) }
					})}
				</p>
				<p class="mt-1 text-xs">{getSurfaceDescription(createdToken.transport_surface)}</p>
				<p class="mt-1 text-xs">{$t('members.tokenWorksOnlyThere')}</p>
			</div>

			<div>
				<p class="mb-1 text-sm font-medium text-fg">
					{$t('members.fullToken')}
				</p>
				<code class="block rounded-control bg-surface p-3 text-xs sm:text-sm text-fg break-all"
					>{createdToken.token}</code
				>
			</div>

			<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
				<div>
					<p class="text-xs text-fg-muted">{$t('members.tokenPrefix')}</p>
					<p class="text-sm font-mono text-fg">
						{createdToken.token_prefix}
					</p>
				</div>
				<div>
					<p class="text-xs text-fg-muted">{$t('common.createdAt')}</p>
					<p class="text-sm text-fg">
						{formatDate(createdToken.created_at)}
					</p>
				</div>
			</div>

			<div class="flex justify-end gap-2 pt-2">
				<Button variant="secondary" onclick={closeCreatedTokenModal}>{$t('common.close')}</Button>
				<Button variant="primary" onclick={copyCreatedToken}>{$t('members.copyToken')}</Button>
			</div>
		</div>
	{/if}
</Modal>

<BotAccessDialog bind:open={showAccessDialog} {workspaceId} bot={accessBot} onsaved={loadBots} />
