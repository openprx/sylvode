<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { setLocale } from '#lib/i18n/index.js';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { authApi, type User } from '#lib/api/auth.js';
	import { authStore } from '#lib/stores/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import { setTheme, theme, type Theme } from '#lib/theme.js';
	import Card from '#lib/components/Card.svelte';
	import Input from '#lib/components/Input.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import ConnectedAppsPersonal from '#lib/oauth/ConnectedAppsPersonal.svelte';
	import NotificationPreferences from '#lib/notifications/NotificationPreferences.svelte';
	import { locale, t } from 'svelte-i18n';

	let loading = $state(true);
	let savingProfile = $state(false);
	let savingPassword = $state(false);
	let currentTheme = $state<Theme>('light');

	let profileForm = $state({
		name: '',
		email: '',
		avatarUrl: ''
	});
	let passwordForm = $state({
		currentPassword: '',
		newPassword: '',
		confirmPassword: ''
	});

	onMount(async () => {
		currentTheme = get(theme);
		const response = await authApi.me();
		if (response.code !== 0) {
			toast.error(describeResult(response));
			loading = false;
			return;
		}

		if (response.data) {
			const user = response.data.user as User;
			profileForm = {
				name: user.name || '',
				email: user.email || '',
				avatarUrl: user.avatar_url || ''
			};
			authStore.setUser(user);
		}

		loading = false;
	});

	async function saveProfile() {
		savingProfile = true;
		const response = await authApi.updateProfile({
			name: profileForm.name,
			email: profileForm.email,
			avatar_url: profileForm.avatarUrl || null
		});

		if (response.code === 0 && response.data?.user) {
			const user = response.data.user;
			profileForm = {
				name: user.name || '',
				email: user.email || '',
				avatarUrl: user.avatar_url || ''
			};
			authStore.setUser(user);
			toast.success($t('settings.profileSaved'));
		} else {
			toast.error(describeResult(response));
		}
		savingProfile = false;
	}

	async function updatePassword() {
		if (!passwordForm.currentPassword || !passwordForm.newPassword) {
			toast.error($t('settings.passwordIncomplete'));
			return;
		}
		if (passwordForm.newPassword.length < 8) {
			toast.error($t('settings.passwordLength'));
			return;
		}
		if (passwordForm.newPassword !== passwordForm.confirmPassword) {
			toast.error($t('settings.passwordMismatch'));
			return;
		}

		savingPassword = true;
		const response = await authApi.updatePassword({
			current_password: passwordForm.currentPassword,
			new_password: passwordForm.newPassword
		});

		if (response.code === 0) {
			passwordForm = { currentPassword: '', newPassword: '', confirmPassword: '' };
			toast.success($t('settings.passwordSaved'));
		} else {
			toast.error(describeResult(response));
		}
		savingPassword = false;
	}

	function handleThemeChange(event: Event) {
		const next = (event.target as HTMLSelectElement).value as Theme;
		currentTheme = next;
		setTheme(next);
	}
</script>

<div class="mx-auto max-w-7xl space-y-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">{$t('settings.title')}</h1>
		<p class="mt-1 text-sm text-fg-muted">{$t('settings.subtitle')}</p>
	</div>

	{#if loading}
		<div class="rounded-card border border-line bg-raised p-6 text-fg-muted">
			{$t('common.loading')}
		</div>
	{:else}
		<Card>
			<h2 class="text-lg font-semibold text-fg">
				{$t('settings.profile')}
			</h2>
			<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-2">
				<Input label={$t('admin.name')} bind:value={profileForm.name} />
				<Input label={$t('auth.email')} type="email" bind:value={profileForm.email} />
				<div class="md:col-span-2">
					<Input
						label={$t('settings.avatarUrl')}
						type="url"
						bind:value={profileForm.avatarUrl}
						placeholder="https://..."
					/>
				</div>
			</div>
			<div class="mt-4">
				<label class="mb-1 block text-sm font-medium text-fg" for="language"
					>{$t('settings.language')}</label
				>
				<select
					id="language"
					class="block w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					onchange={(e) => setLocale((e.target as HTMLSelectElement).value)}
					value={$locale}
				>
					<option value="zh">{$t('settings.zh')}</option>
					<option value="en">{$t('settings.en')}</option>
				</select>
			</div>
			<div class="mt-4">
				<label class="mb-1 block text-sm font-medium text-fg" for="theme"
					>{$t('settings.theme')}</label
				>
				<select
					id="theme"
					class="block w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
					onchange={handleThemeChange}
					value={currentTheme}
				>
					<option value="light">{$t('settings.themeLight')}</option>
					<option value="dark">{$t('settings.themeDark')}</option>
					<option value="system">{$t('settings.themeSystem')}</option>
				</select>
			</div>
			<div class="mt-4 flex justify-end">
				<Button variant="primary" loading={savingProfile} onclick={saveProfile}
					>{$t('settings.saveProfile')}</Button
				>
			</div>
		</Card>

		<Card>
			<h2 class="text-lg font-semibold text-fg">
				{$t('settings.changePassword')}
			</h2>
			<div class="mt-4 grid grid-cols-1 gap-4 md:grid-cols-3">
				<Input
					type="password"
					label={$t('settings.currentPassword')}
					bind:value={passwordForm.currentPassword}
				/>
				<Input
					type="password"
					label={$t('settings.newPassword')}
					bind:value={passwordForm.newPassword}
				/>
				<Input
					type="password"
					label={$t('settings.confirmPassword')}
					bind:value={passwordForm.confirmPassword}
				/>
			</div>
			<div class="mt-4 flex justify-end">
				<Button variant="primary" loading={savingPassword} onclick={updatePassword}
					>{$t('settings.updatePassword')}</Button
				>
			</div>
		</Card>

		<NotificationPreferences />

		<ConnectedAppsPersonal />
	{/if}
</div>
