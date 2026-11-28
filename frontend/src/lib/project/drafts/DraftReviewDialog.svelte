<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Dialog from '#lib/ui/dialog/Dialog.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import Textarea from '#lib/ui/textarea/Textarea.svelte';
	import { draftsApi } from './api';
	import { canApprove, issueLine, riskTone, staleDigest, statusTone, targetLine } from './view';
	import type { Draft } from './wire';

	/**
	 * Review of one draft: the validation report, what the artefact could ask for (capabilities),
	 * what it would expose to visitors of published sites, and the spec itself. Approve sends the
	 * digest of what is on screen, so a draft that changed meanwhile is refused and shown again.
	 */
	interface Props {
		draftId: string;
		isAdmin: boolean;
		onclose: () => void;
	}
	let { draftId, isAdmin, onclose }: Props = $props();

	let draft = $state.raw<Draft | null>(null);
	let phase = $state<'loading' | 'ready' | 'failed'>('loading');
	let busy = $state(false);
	let problem = $state('');
	let stale = $state(false);
	let rejecting = $state(false);
	let comment = $state('');

	async function load(): Promise<void> {
		phase = 'loading';
		const result = await draftsApi.get(draftId);
		if (result.code === 0 && result.data) {
			draft = result.data;
			phase = 'ready';
		} else {
			problem = describeResult(result);
			phase = 'failed';
		}
	}

	onMount(() => {
		void load();
	});

	async function approve(): Promise<void> {
		if (!draft || busy) return;
		busy = true;
		problem = '';
		const result = await draftsApi.approve(draft.id, draft.review_digest);
		busy = false;
		if (result.code === 0) {
			toast.success($t('drafts.review.approved'));
			onclose();
			return;
		}
		if (result.error_code === 'draft_stale' || staleDigest(result.details) !== null) {
			stale = true;
			await load();
			return;
		}
		problem = describeResult(result, $t('drafts.review.approveFailed'));
		await load();
	}

	async function reject(): Promise<void> {
		if (!draft || busy || comment.trim() === '') return;
		busy = true;
		problem = '';
		const result = await draftsApi.reject(draft.id, comment.trim());
		busy = false;
		if (result.code === 0) {
			toast.success($t('drafts.review.rejected'));
			onclose();
		} else problem = describeResult(result, $t('drafts.review.rejectFailed'));
	}

	const pending = $derived(draft?.status === 'pending');
	const specText = $derived(draft ? JSON.stringify(draft.spec, null, 2) : '');
</script>

<Dialog
	open={true}
	title={$t('drafts.review.title')}
	size="wide"
	{onclose}
	testId="draft-review-dialog"
>
	{#if phase === 'loading' && !draft}
		<StateBlock kind="loading" thing={$t('drafts.thing')} />
	{:else if phase === 'failed' || !draft}
		<StateBlock
			kind="loadFailed"
			thing={$t('drafts.thing')}
			message={problem || undefined}
			action={{ run: () => void load() }}
		/>
	{:else}
		<div class="review">
			<p class="meta">
				<StatusChip tone={statusTone(draft.status)}
					>{$t(`drafts.status.${draft.status}`)}</StatusChip
				>
				<span
					>{$t(`drafts.kind.${draft.kind}`)}: {targetLine(draft, (key, values) =>
						$t(key, { values })
					)}</span
				>
				<span
					>{$t('drafts.review.by', {
						values: { name: draft.author.name ?? draft.author.id }
					})}</span
				>
			</p>

			{#if draft.note}
				<p class="note" data-testid="draft-note">{draft.note}</p>
			{/if}

			{#if stale}
				<SecurityWarning title={$t('drafts.review.staleTitle')}>
					{$t('drafts.review.staleBody')}
				</SecurityWarning>
			{/if}
			{#if problem}
				<SecurityWarning tone="danger" title={$t('drafts.review.problem')}
					>{problem}</SecurityWarning
				>
			{/if}

			<section aria-labelledby="draft-validation">
				<h2 id="draft-validation">{$t('drafts.review.validation')}</h2>
				{#if draft.validation.valid}
					<p data-testid="draft-valid">{$t('drafts.review.valid')}</p>
				{:else}
					<SecurityWarning tone="danger" title={$t('drafts.review.invalid')}>
						{$t('drafts.review.invalidBody')}
					</SecurityWarning>
				{/if}
				{#if draft.validation.errors.length > 0}
					<ul class="issues" data-testid="draft-errors">
						{#each draft.validation.errors as issue (issueLine(issue))}
							<li><code>{issueLine(issue)}</code></li>
						{/each}
					</ul>
				{/if}
				{#if (draft.validation.warnings ?? []).length > 0}
					<p class="sub">{$t('drafts.review.warnings')}</p>
					<ul class="issues">
						{#each draft.validation.warnings ?? [] as issue (issueLine(issue))}
							<li><code>{issueLine(issue)}</code></li>
						{/each}
					</ul>
				{/if}
			</section>

			<section aria-labelledby="draft-capabilities">
				<h2 id="draft-capabilities">{$t('drafts.review.capabilities')}</h2>
				{#if draft.capability_diff.length === 0}
					<p>{$t('drafts.review.noCapabilities')}</p>
				{:else}
					<table class="diff" data-testid="draft-capabilities">
						<thead>
							<tr>
								<th scope="col">{$t('drafts.review.capability')}</th>
								<th scope="col">{$t('drafts.review.selector')}</th>
								<th scope="col">{$t('drafts.review.risk')}</th>
							</tr>
						</thead>
						<tbody>
							{#each draft.capability_diff as item (item.capability + JSON.stringify(item.selector))}
								<tr>
									<td><code>{item.capability}</code></td>
									<td><code>{JSON.stringify(item.selector)}</code></td>
									<td>
										<StatusChip tone={riskTone(item.risk)}>{item.risk}</StatusChip>
										{#if item.new}<StatusChip tone="warning">{$t('drafts.review.new')}</StatusChip
											>{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			</section>

			<section aria-labelledby="draft-public">
				<h2 id="draft-public">{$t('drafts.review.public')}</h2>
				{#if draft.public.length === 0}
					<p data-testid="draft-public-none">{$t('drafts.review.noPublic')}</p>
				{:else}
					<SecurityWarning tone="danger" title={$t('drafts.review.publicWarning')}>
						<ul class="issues">
							{#each draft.public as item (JSON.stringify(item))}
								<li><code>{JSON.stringify(item)}</code></li>
							{/each}
						</ul>
					</SecurityWarning>
				{/if}
			</section>

			<section aria-labelledby="draft-spec">
				<h2 id="draft-spec">{$t('drafts.review.spec')}</h2>
				<pre class="spec" data-testid="draft-spec">{specText}</pre>
			</section>

			{#if draft.review?.comment}
				<p class="note">{$t('drafts.review.comment')}: {draft.review.comment}</p>
			{/if}
			{#if draft.result}
				<p class="note"><code>{JSON.stringify(draft.result)}</code></p>
			{/if}

			{#if rejecting}
				<Textarea
					label={$t('drafts.review.rejectReason')}
					bind:value={comment}
					maxlength={2000}
					rows={3}
				/>
			{/if}
		</div>
	{/if}

	{#snippet actions()}
		<Button variant="text" onclick={onclose}>{$t('drafts.review.close')}</Button>
		{#if draft && pending && isAdmin}
			{#if rejecting}
				<Button
					variant="danger"
					loading={busy}
					loadingLabel={$t('drafts.review.working')}
					disabled={comment.trim() === ''}
					disabledReason={$t('drafts.review.reasonNeeded')}
					onclick={() => void reject()}>{$t('drafts.review.confirmReject')}</Button
				>
			{:else}
				<Button variant="secondary" onclick={() => (rejecting = true)}>
					{$t('drafts.review.reject')}
				</Button>
				<Button
					variant="primary"
					loading={busy}
					loadingLabel={$t('drafts.review.working')}
					disabled={!canApprove(draft, true)}
					disabledReason={$t('drafts.review.cannotApprove')}
					onclick={() => void approve()}>{$t('drafts.review.approve')}</Button
				>
			{/if}
		{/if}
	{/snippet}
</Dialog>

<style>
	.review {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-height: 70vh;
		overflow: auto;
	}
	.meta {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: center;
		margin: 0;
		font-size: var(--rf-size-secondary);
	}
	.note {
		margin: 0;
		font-size: var(--rf-size-secondary);
	}
	h2 {
		margin: 0 0 var(--rf-space-3);
		font-size: var(--rf-size-secondary);
		font-weight: var(--rf-weight-semibold);
	}
	.sub {
		margin: var(--rf-space-3) 0 0;
		font-size: var(--rf-size-secondary);
		color: var(--rf-text-secondary);
	}
	.issues {
		margin: var(--rf-space-3) 0 0;
		padding-inline-start: var(--rf-space-6);
		font-size: var(--rf-size-secondary);
	}
	.diff {
		width: 100%;
		border-collapse: collapse;
		font-size: var(--rf-size-secondary);
	}
	.diff th,
	.diff td {
		padding: var(--rf-space-3) var(--rf-space-4);
		text-align: start;
		border-bottom: 1px solid var(--rf-border);
		vertical-align: top;
	}
	.spec {
		margin: 0;
		padding: var(--rf-space-4);
		max-height: 240px;
		overflow: auto;
		background: var(--rf-surface);
		border-radius: var(--rf-radius-card);
		font-size: var(--rf-size-meta);
	}
</style>
