<script lang="ts">
	import { describeResult } from '#lib/i18n/describe.js';
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { authStore } from '#lib/stores/auth.js';
	import { vetoApi } from '#lib/api/veto.js';
	import { isAdminUser } from '#lib/utils/auth.js';
	import { trustApi, type Appeal, type AppealStatus } from '#lib/api/trust.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Modal from '#lib/components/Modal.svelte';
	import Input from '#lib/components/Input.svelte';
	import Textarea from '#lib/components/Textarea.svelte';

	let myVetoScopeKeys = $state<Set<string>>(new Set());

	const isProjectAdmin = $derived(isAdminUser($authStore.user));
	const canReview = $derived(isProjectAdmin || myVetoScopeKeys.size > 0);

	let loading = $state(false);
	let saving = $state(false);
	let showCreate = $state(false);
	let statusFilter = $state<'all' | AppealStatus>('all');
	let mineOnly = $state(true);
	let items = $state<Appeal[]>([]);
	let reviewingId = $state<number | null>(null);

	let createForm = $state({
		log_id: '',
		reason: '',
		evidence: ''
	});

	let reviewForm = $state({
		status: 'accepted' as 'accepted' | 'rejected',
		review_note: ''
	});

	onMount(() => {
		void load();
	});

	async function load() {
		loading = true;
		await loadMyVetoScopes();
		const res = await trustApi.listAppeals({ status: statusFilter, mine: mineOnly });
		if (res.code !== 0 || !res.data) {
			toast.error(describeResult(res, get(t)('appeals.loadFailed')));
			items = [];
			loading = false;
			return;
		}
		items = res.data.items;
		loading = false;
	}

	async function loadMyVetoScopes() {
		const meId = $authStore.user?.id;
		if (!meId) {
			myVetoScopeKeys = new Set();
			return;
		}

		const res = await vetoApi.listVetoers();
		if (res.code !== 0 || !res.data) {
			myVetoScopeKeys = new Set();
			return;
		}

		// eslint-disable-next-line svelte/prefer-svelte-reactivity -- local scratch collection, not reactive state
		const next = new Set<string>();
		for (const row of res.data.items || []) {
			if (row.user_id === meId && row.project_id && row.domain) {
				next.add(`${row.project_id}:${row.domain}`);
			}
		}
		myVetoScopeKeys = next;
	}

	function canReviewAppeal(item: Appeal): boolean {
		if (isProjectAdmin) {
			return true;
		}
		if (!item.project_id || !item.domain) {
			return false;
		}
		return myVetoScopeKeys.has(`${item.project_id}:${item.domain}`);
	}

	function statusTone(status: AppealStatus): string {
		switch (status) {
			case 'accepted':
				return 'bg-surface text-fg';
			case 'rejected':
				return 'bg-danger-soft text-danger';
			default:
				return 'bg-warning-soft text-warning';
		}
	}

	async function createAppeal() {
		const logId = Number(createForm.log_id);
		if (!Number.isInteger(logId) || logId <= 0 || !createForm.reason.trim()) {
			toast.error(get(t)('appeals.invalidCreateForm'));
			return;
		}

		saving = true;
		let evidence: unknown = undefined;
		if (createForm.evidence.trim()) {
			try {
				evidence = JSON.parse(createForm.evidence);
			} catch {
				toast.error(get(t)('appeals.invalidEvidence'));
				saving = false;
				return;
			}
		}

		const res = await trustApi.createAppeal({
			log_id: logId,
			reason: createForm.reason.trim(),
			evidence
		});
		saving = false;
		if (res.code !== 0) {
			toast.error(describeResult(res, get(t)('appeals.createFailed')));
			return;
		}
		toast.success(get(t)('appeals.createSuccess'));
		showCreate = false;
		createForm = { log_id: '', reason: '', evidence: '' };
		await load();
	}

	async function reviewAppeal(item: Appeal, status: 'accepted' | 'rejected') {
		reviewingId = item.id;
		const res = await trustApi.updateAppeal(item.id, {
			status,
			review_note: reviewForm.review_note.trim() || undefined
		});
		reviewingId = null;
		if (res.code !== 0) {
			toast.error(describeResult(res, get(t)('appeals.reviewFailed')));
			return;
		}
		toast.success(get(t)('appeals.reviewSuccess'));
		reviewForm = { status: 'accepted', review_note: '' };
		await load();
	}

	async function removeAppeal(item: Appeal) {
		if (
			!(await confirmAction({
				tone: 'destructive',
				title: get(t)('appeals.deleteTitle'),
				body: get(t)('appeals.deleteBody'),
				confirmLabel: get(t)('appeals.deleteAction')
			}))
		) {
			return;
		}
		const res = await trustApi.deleteAppeal(item.id);
		if (res.code !== 0) {
			toast.error(describeResult(res, get(t)('appeals.deleteFailed')));
			return;
		}
		toast.success(get(t)('appeals.deleteSuccess'));
		await load();
	}

	function appealStatusLabel(status: AppealStatus): string {
		return get(t)(`appeals.status.${status}`) || status;
	}
</script>

<svelte:head>
	<title>{$t('pageTitle.trustAppeals')}</title>
</svelte:head>

<div class="mx-auto max-w-6xl space-y-4">
	<div class="rounded-card border border-line bg-raised p-4">
		<div class="mb-4 flex flex-wrap items-center justify-between gap-3">
			<div>
				<h1 class="text-xl font-semibold text-fg">
					{$t('appeals.title')}
				</h1>
				<p class="text-sm text-fg-muted">{$t('appeals.subtitle')}</p>
			</div>
			<Button variant="primary" onclick={() => (showCreate = true)}>{$t('appeals.create')}</Button>
		</div>

		<div class="mb-4 flex flex-wrap gap-2">
			<select
				bind:value={statusFilter}
				onchange={load}
				class="rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<option value="all">{$t('appeals.status.all')}</option>
				<option value="pending">{$t('appeals.status.pending')}</option>
				<option value="accepted">{$t('appeals.status.accepted')}</option>
				<option value="rejected">{$t('appeals.status.rejected')}</option>
			</select>
			<label
				class="inline-flex items-center gap-2 rounded-control border border-input bg-field px-3 py-2 text-sm"
			>
				<input type="checkbox" bind:checked={mineOnly} onchange={load} />
				{$t('appeals.mineOnly')}
			</label>
		</div>

		{#if loading}
			<div class="py-8 text-center text-fg-muted">{$t('common.loading')}</div>
		{:else if items.length === 0}
			<div class="py-8 text-center text-fg-muted">{$t('appeals.empty')}</div>
		{:else}
			<div class="space-y-2">
				{#each items as item (item.id)}
					<div class="rounded-card border border-line p-3">
						<div class="flex flex-wrap items-start justify-between gap-3">
							<div class="min-w-0 flex-1">
								<div class="flex items-center gap-2">
									<span class="font-medium text-fg"
										>{$t('appeals.logId', { values: { id: item.log_id } })}</span
									>
									<span class={`rounded px-2 py-0.5 text-xs ${statusTone(item.status)}`}
										>{appealStatusLabel(item.status)}</span
									>
								</div>
								<p class="mt-1 text-sm text-fg">{item.reason}</p>
								<p class="mt-1 text-xs text-fg-muted">
									{$t('appeals.meta', {
										values: {
											appellant: item.appellant_id,
											created: new Date(item.created_at).toLocaleString()
										}
									})}
								</p>
								{#if item.review_note}
									<p class="mt-1 text-xs text-fg-muted">
										{$t('appeals.reviewNote')}: {item.review_note}
									</p>
								{/if}
							</div>

							<div class="flex flex-wrap gap-2">
								{#if item.status === 'pending' && canReviewAppeal(item)}
									<Button
										variant="primary"
										size="sm"
										onclick={() => reviewAppeal(item, 'accepted')}
										loading={reviewingId === item.id}>{$t('appeals.accept')}</Button
									>
									<Button
										size="sm"
										variant="danger-quiet"
										onclick={() => reviewAppeal(item, 'rejected')}
										loading={reviewingId === item.id}>{$t('appeals.reject')}</Button
									>
								{/if}
								{#if item.status === 'pending' && item.appellant_id === $authStore.user?.id}
									<Button size="sm" variant="secondary" onclick={() => removeAppeal(item)}
										>{$t('common.delete')}</Button
									>
								{/if}
							</div>
						</div>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</div>

<Modal bind:open={showCreate} title={$t('appeals.createTitle')}>
	<form
		onsubmit={(event) => {
			event.preventDefault();
			void createAppeal();
		}}
		class="space-y-3"
	>
		<Input type="number" label={$t('appeals.logIdInput')} bind:value={createForm.log_id} required />
		<Textarea label={$t('appeals.reasonInput')} bind:value={createForm.reason} rows={4} required />
		<Textarea
			label={$t('appeals.evidenceInput')}
			bind:value={createForm.evidence}
			rows={3}
			hint={$t('appeals.evidenceHint')}
		/>
		{#if canReview}
			<Textarea label={$t('appeals.reviewNote')} bind:value={reviewForm.review_note} rows={2} />
		{/if}
		<div class="flex justify-end gap-2">
			<Button variant="secondary" onclick={() => (showCreate = false)}>{$t('common.cancel')}</Button
			>
			<Button variant="primary" type="submit" loading={saving}>{$t('common.submit')}</Button>
		</div>
	</form>
</Modal>
