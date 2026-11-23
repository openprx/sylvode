<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { t } from 'svelte-i18n';
	import { apiClient } from '#lib/api/client.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';

	interface Props {
		status: number;
		/** The error's own message; shown only for failures other than "not found". */
		message?: string;
	}

	let { status, message = '' }: Props = $props();

	// Home is the workspace list for a signed-in user and the sign-in page otherwise.
	function goHome() {
		void goto(apiClient.getToken() ? resolve('/(app)/workspace') : resolve('/(auth)/auth/login'));
	}
</script>

<div class="mx-auto max-w-xl py-8" data-testid="route-error">
	{#if status === 404}
		<StateBlock
			kind="notFound"
			object={$t('errorPage.object')}
			message={$t('errorPage.notFoundBody')}
			action={{ label: $t('errorPage.home'), run: goHome, primary: true }}
			data-testid="route-error-not-found"
		/>
	{:else}
		<StateBlock
			kind="loadFailed"
			message={message || $t('errorPage.failedBody')}
			action={{ label: $t('errorPage.home'), run: goHome }}
			data-testid="route-error-failed"
		/>
	{/if}
</div>
