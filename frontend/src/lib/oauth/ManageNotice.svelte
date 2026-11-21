<script lang="ts">
	/**
	 * The inline result and error area of a "Connected applications" section (design rules: errors
	 * are shown on the page, not as toasts). A new error takes focus so a screen reader reads it
	 * and a keyboard user is already there.
	 */
	import { t } from 'svelte-i18n';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';

	interface Props {
		success?: string | null;
		/** Translation key of the error message. */
		errorKey?: string | null;
		onretry?: () => void;
		testIdPrefix: string;
	}

	let { success = null, errorKey = null, onretry, testIdPrefix }: Props = $props();

	let errorBox = $state<HTMLElement | null>(null);

	$effect(() => {
		if (errorKey && errorBox) errorBox.focus();
	});
</script>

{#if errorKey}
	<div class="notice" tabindex="-1" bind:this={errorBox} data-testid={`${testIdPrefix}-error`}>
		<TipBar
			tone="danger"
			action={onretry ? { label: $t('oauth.manage.retry'), run: onretry } : undefined}
		>
			{$t(errorKey)}
		</TipBar>
	</div>
{:else if success}
	<div class="notice" data-testid={`${testIdPrefix}-result`}>
		<TipBar tone="success">{success}</TipBar>
	</div>
{/if}

<style>
	.notice {
		outline: none;
	}
</style>
