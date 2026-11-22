<script lang="ts">
	import '#lib/i18n/index.js';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { apiClient } from '#lib/api/client.js';
	import { authApi } from '#lib/api/auth.js';
	import { authStore } from '#lib/stores/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import { safeReturnTo } from '#lib/oauth/return-to.js';
	import { t } from 'svelte-i18n';
	import BrandMark from '#lib/shell/brand/BrandMark.svelte';

	let email = '';
	let password = '';
	let loading = false;

	async function handleLogin() {
		if (!email || !password) {
			toast.error($t('auth.required'));
			return;
		}

		loading = true;
		const response = await authApi.login({ email, password });
		if (!apiClient.isAuthResultCurrent(response)) {
			loading = false;
			return;
		}

		if (response.code !== 0) {
			toast.error(response.message);
		} else if (response.data) {
			authStore.setUser(response.data.user);
			toast.success($t('auth.loginSuccess'));
			// Only the consent page's own address is accepted as a return target (contract 14.4).
			const returnTo = safeReturnTo(page.url.searchParams.get('return_to'));
			if (returnTo) {
				// eslint-disable-next-line svelte/no-navigation-without-resolve -- validated same-origin path
				goto(returnTo);
			} else {
				goto(resolve('/(app)/workspace'));
			}
		}

		loading = false;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.login')}</title>
</svelte:head>

<div class="min-h-screen flex items-center justify-center bg-canvas px-4">
	<div class="max-w-md w-full space-y-8">
		<div class="text-center">
			<div class="mb-4 flex justify-center">
				<BrandMark decorative size={40} />
			</div>
			<h1 class="text-3xl font-semibold text-fg">Sylvode</h1>
			<p class="mt-2 text-sm text-fg-muted">{$t('auth.subtitle')}</p>
		</div>

		<form class="mt-8 space-y-6 rounded-card bg-raised p-8" on:submit|preventDefault={handleLogin}>
			<div class="space-y-4">
				<div>
					<label for="email" class="block text-sm font-medium text-fg">{$t('auth.email')}</label>
					<input
						id="email"
						type="email"
						bind:value={email}
						required
						class="mt-1 block w-full rounded-control border border-input bg-field px-3 py-2 text-fg"
						placeholder={$t('placeholders.emailPlaceholder')}
					/>
				</div>

				<div>
					<label for="password" class="block text-sm font-medium text-fg"
						>{$t('auth.password')}</label
					>
					<input
						id="password"
						type="password"
						bind:value={password}
						required
						class="mt-1 block w-full rounded-control border border-input bg-field px-3 py-2 text-fg"
						placeholder={$t('placeholders.passwordPlaceholder')}
					/>
				</div>
			</div>

			<button
				type="submit"
				disabled={loading}
				class="w-full flex justify-center py-2 px-4 border border-transparent rounded-control text-sm font-medium text-on-primary rf-filled bg-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
			>
				{loading ? $t('auth.loginLoading') : $t('auth.login')}
			</button>
		</form>
	</div>
</div>
