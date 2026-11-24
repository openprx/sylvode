<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { describeResult } from '#lib/i18n/describe.js';
	import { vetoApi, type VetoerScope } from '#lib/api/veto.js';
	import Modal from '#lib/components/Modal.svelte';
	import { toast } from '#lib/stores/toast.js';
	import { MIN_VETO_REASON, reasonLength, reasonProblem, vetoDomainsFor } from './veto-rules.js';

	interface Props {
		proposalId: string;
		status: string;
		domains: readonly string[];
		userId: string;
	}

	let { proposalId, status, domains, userId }: Props = $props();

	let vetoers = $state<VetoerScope[]>([]);
	let open = $state(false);
	let domain = $state('');
	let reason = $state('');
	let reasonError = $state('');
	let submitError = $state('');
	let saving = $state(false);

	// The button shows only for a caller who is listed as a vetoer in a domain of this proposal.
	const choices = $derived(vetoDomainsFor(status, domains, vetoers, userId));
	const count = $derived(reasonLength(reason));

	onMount(async () => {
		const response = await vetoApi.listVetoers();
		if (response.code === 0 && response.data) vetoers = response.data.items ?? [];
	});

	function openDialog() {
		domain = choices[0] ?? '';
		reason = '';
		reasonError = '';
		submitError = '';
		open = true;
	}

	async function submit() {
		if (saving) return;
		const problem = reasonProblem(reason);
		if (problem) {
			reasonError = $t(
				problem === 'empty' ? 'vetoExercise.enterReason' : 'vetoExercise.reasonShort',
				{
					values: { min: MIN_VETO_REASON }
				}
			);
			return;
		}
		saving = true;
		submitError = '';
		const response = await vetoApi.exercise(proposalId, { reason: reason.trim(), domain });
		saving = false;
		if (response.code !== 0) {
			submitError = describeResult(response, $t('vetoExercise.failed'));
			return;
		}
		open = false;
		toast.success($t('vetoExercise.done'));
		await goto(resolve('/(app)/proposals/[id]/veto', { id: proposalId }));
	}
</script>

{#if choices.length > 0}
	<button
		type="button"
		onclick={openDialog}
		data-testid="exercise-veto"
		class="w-full rounded-control border border-danger px-3 py-2 text-sm text-danger hover:bg-danger-soft"
	>
		{$t('vetoExercise.action')}
	</button>
{/if}

{#snippet footer()}
	<button
		type="button"
		onclick={() => (open = false)}
		class="rounded-control border border-fg px-4 py-2 text-sm text-fg hover:bg-surface"
	>
		{$t('common.cancel')}
	</button>
	<button
		type="button"
		onclick={submit}
		disabled={saving}
		data-testid="exercise-veto-submit"
		class="rounded-control rf-filled bg-danger px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
	>
		{saving ? $t('vetoExercise.submitting') : $t('vetoExercise.submit')}
	</button>
{/snippet}

<Modal bind:open title={$t('vetoExercise.title')} maxWidthClass="max-w-lg" {footer}>
	<div class="space-y-4">
		<p class="text-sm text-fg-muted">{$t('vetoExercise.intro')}</p>
		{#if choices.length > 1}
			<div>
				<label for="veto-domain" class="mb-1 block text-sm font-medium text-fg"
					>{$t('vetoExercise.domain')}</label
				>
				<select
					id="veto-domain"
					bind:value={domain}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					{#each choices as key (key)}
						<option value={key}>{key}</option>
					{/each}
				</select>
			</div>
		{:else}
			<p class="text-sm text-fg">
				<span class="text-fg-muted">{$t('vetoExercise.domain')}:</span>
				{domain}
			</p>
		{/if}
		<div>
			<label for="veto-reason" class="mb-1 block text-sm font-medium text-fg"
				>{$t('vetoExercise.reason')}</label
			>
			<textarea
				id="veto-reason"
				bind:value={reason}
				rows="5"
				oninput={() => (reasonError = '')}
				aria-invalid={reasonError ? 'true' : undefined}
				aria-describedby="veto-reason-hint"
				class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
			></textarea>
			<p id="veto-reason-hint" class="mt-1 text-xs text-fg-muted">
				{$t('vetoExercise.reasonCount', { values: { count, min: MIN_VETO_REASON } })}
			</p>
			{#if reasonError}
				<p role="alert" class="mt-1 text-sm text-danger">{reasonError}</p>
			{/if}
		</div>
		{#if submitError}
			<p role="alert" class="text-sm text-danger">{submitError}</p>
		{/if}
	</div>
</Modal>
