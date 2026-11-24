<svelte:options runes />

<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import '#lib/i18n/index.js';
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import { authApi } from '#lib/api/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import { setupBody, validateSetup } from '#lib/setup/rules.js';
	import BrandMark from '#lib/shell/brand/BrandMark.svelte';
	import Button from '#lib/ui/button/Button.svelte';
	import Input from '#lib/components/Input.svelte';

	let checking = $state(true);
	let statusError = $state('');
	let saving = $state(false);
	let formError = $state('');
	let form = $state({ name: '', email: '', password: '', confirm: '' });

	onMount(async () => {
		const response = await authApi.setupStatus();
		if (response.code !== 0) {
			statusError = describeResult(response);
		} else if (!response.data?.needs_setup) {
			goto(resolve('/(auth)/auth/login'));
			return;
		}
		checking = false;
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		const problem = validateSetup(form);
		formError = problem ? $t(problem) : '';
		if (problem) return;

		saving = true;
		const response = await authApi.register(setupBody(form));
		saving = false;
		if (response.code !== 0) {
			formError = describeResult(response);
			return;
		}
		toast.success($t('setup.created'));
		goto(resolve('/(auth)/auth/login'));
	}
</script>

<svelte:head>
	<title>{$t('setup.pageTitle')}</title>
</svelte:head>

<div class="min-h-screen flex items-center justify-center bg-canvas px-4">
	<div class="max-w-md w-full space-y-8">
		<div class="text-center">
			<div class="mb-4 flex justify-center">
				<BrandMark decorative size={40} />
			</div>
			<h1 class="text-3xl font-semibold text-fg">{$t('setup.title')}</h1>
			<p class="mt-2 text-sm text-fg-muted">{$t('setup.subtitle')}</p>
		</div>

		{#if checking && !statusError}
			<p class="text-center text-sm text-fg-muted" role="status">{$t('setup.checking')}</p>
		{:else if statusError}
			<div class="rounded-card bg-raised p-8 space-y-4 text-center">
				<p class="text-sm text-danger" role="alert">{statusError}</p>
				<a
					class="inline-flex min-h-11 items-center text-sm text-fg underline"
					href={resolve('/(auth)/auth/login')}>{$t('setup.backToLogin')}</a
				>
			</div>
		{:else}
			<form class="rounded-card bg-raised p-8 space-y-4" onsubmit={submit}>
				<Input label={$t('setup.name')} bind:value={form.name} required />
				<Input type="email" label={$t('setup.email')} bind:value={form.email} required />
				<Input
					type="password"
					label={$t('setup.password')}
					hint={$t('setup.passwordHint')}
					bind:value={form.password}
					required
					minlength={8}
				/>
				<Input
					type="password"
					label={$t('setup.confirm')}
					bind:value={form.confirm}
					required
					minlength={8}
				/>
				{#if formError}
					<p class="text-sm text-danger" role="alert">{formError}</p>
				{/if}
				<div class="flex justify-end">
					<Button variant="primary" type="submit" loading={saving}>
						{saving ? $t('setup.submitting') : $t('setup.submit')}
					</Button>
				</div>
			</form>
		{/if}
	</div>
</div>
