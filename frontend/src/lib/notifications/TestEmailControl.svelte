<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { adminApi } from '#lib/api/admin.js';
	import { describeResult } from '#lib/i18n/describe.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Card from '#lib/components/Card.svelte';
	import { testEmailOutcome, type TestEmailOutcome } from './test-email.js';

	let sending = $state(false);
	let outcome = $state<TestEmailOutcome | null>(null);

	async function send() {
		if (sending) return;
		sending = true;
		outcome = testEmailOutcome(await adminApi.sendTestEmail());
		sending = false;
	}
</script>

<Card>
	<h2 class="text-lg font-semibold text-fg">{$t('notificationPrefs.testEmail.title')}</h2>
	<p class="mt-1 text-sm text-fg-muted">{$t('notificationPrefs.testEmail.description')}</p>
	<div class="mt-4 flex flex-wrap items-center gap-3">
		<Button variant="secondary" loading={sending} onclick={send} data-testid="send-test-email"
			>{$t('notificationPrefs.testEmail.send')}</Button
		>
		<span class="text-sm text-fg-muted" role="status" data-testid="test-email-outcome">
			{#if outcome?.kind === 'queued'}
				{$t('notificationPrefs.testEmail.queued')}
			{:else if outcome?.kind === 'not_set_up'}
				{$t('notificationPrefs.emailUnavailable')}
			{:else if outcome?.kind === 'forbidden'}
				{$t('notificationPrefs.testEmail.forbidden')}
			{:else if outcome?.kind === 'failed'}
				{describeResult(outcome.result, $t('notificationPrefs.testEmail.failed'))}
			{/if}
		</span>
	</div>
</Card>
