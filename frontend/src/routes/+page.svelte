<script>
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { authStore } from '#lib/stores/auth.js';
	import { get } from 'svelte/store';

	onMount(() => {
		const user = get(authStore).user;
		if (user) {
			// Logged in: redirect to workspace selector
			goto(resolve('/(app)/workspace'));
		} else {
			// Not logged in: redirect to login page
			goto(resolve('/(auth)/auth/login'));
		}
	});
</script>

<div class="container">
	<h1>{$t('landing.title')}</h1>
	<p>{$t('landing.subtitle')}</p>
	<p>{$t('landing.redirecting')}</p>
</div>

<style>
	.container {
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		min-height: 100vh;
		text-align: center;
	}

	h1 {
		font-size: 3rem;
		font-weight: bold;
		margin-bottom: 1rem;
	}

	p {
		font-size: 1.2rem;
		color: var(--rf-secondary);
	}
</style>
