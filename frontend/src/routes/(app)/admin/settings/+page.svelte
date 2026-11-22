<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { authApi } from '#lib/api/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import { isAdminUser } from '#lib/utils/auth.js';
	import Card from '#lib/components/Card.svelte';
	import Input from '#lib/components/Input.svelte';
	import ConnectedAppsAdmin from '#lib/oauth/ConnectedAppsAdmin.svelte';

	let basicSettings = $state({
		siteName: 'Sylvode',
		siteUrl: 'https://sylvode.local',
		allowWorkspaceCreate: 'admin_only'
	});
	let securitySettings = $state({
		passwordMinLength: '8',
		requireStrongPassword: true,
		sessionHours: '24'
	});

	onMount(async () => {
		const meResponse = await authApi.me();
		if (meResponse.code !== 0 || !isAdminUser(meResponse.data?.user ?? null)) {
			toast.error(get(t)('admin.settingsAdminOnly'));
			goto('/workspace');
		}
	});
</script>

<div class="mx-auto max-w-5xl space-y-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">
			{$t('admin.systemSettings')}
		</h1>
		<p class="mt-1 text-sm text-fg-muted">{$t('admin.settingsDescription')}</p>
	</div>

	<Card>
		<h2 class="text-lg font-semibold text-fg">
			{$t('admin.basicSettings')}
		</h2>
		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
			<Input label={$t('admin.siteName')} bind:value={basicSettings.siteName} disabled />
			<Input label={$t('admin.siteUrl')} bind:value={basicSettings.siteUrl} disabled />
			<div class="md:col-span-2">
				<label class="mb-1 block text-sm font-medium text-fg" for="allowWorkspaceCreate"
					>{$t('admin.workspaceCreatePermission')}</label
				>
				<select
					id="allowWorkspaceCreate"
					bind:value={basicSettings.allowWorkspaceCreate}
					disabled
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="admin_only">{$t('admin.adminOnly')}</option>
					<option value="all_users">{$t('admin.allUsers')}</option>
				</select>
			</div>
		</div>
	</Card>

	<Card>
		<h2 class="text-lg font-semibold text-fg">
			{$t('admin.securitySettings')}
		</h2>
		<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-3">
			<Input
				label={$t('admin.minPasswordLength')}
				type="number"
				bind:value={securitySettings.passwordMinLength}
				disabled
			/>
			<Input
				label={$t('admin.sessionHours')}
				type="number"
				bind:value={securitySettings.sessionHours}
				disabled
			/>
			<label
				class="flex items-center gap-2 rounded-control border border-line px-3 py-2 text-sm text-fg"
			>
				<input type="checkbox" bind:checked={securitySettings.requireStrongPassword} disabled />
				{$t('admin.strongPasswordPolicy')}
			</label>
		</div>
	</Card>

	<ConnectedAppsAdmin />

	<div class="rounded-card border border-line bg-raised px-4 py-3 text-sm text-fg-muted">
		{$t('admin.settingsReadOnly')}
	</div>
</div>
