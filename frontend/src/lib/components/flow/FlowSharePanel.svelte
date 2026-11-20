<script lang="ts">
	// Share panel (`ui-surface-v1.md` "职责边界" Share panel). All state and every rule lives in
	// `FlowSharePanelController` (`lib/flow/share-panel.ts`); this component only renders it and
	// forwards user actions.
	import { flowApi } from '#lib/api/flow.js';
	import { t } from 'svelte-i18n';
	import Modal from '#lib/components/Modal.svelte';
	import {
		FLOW_GRANT_LEVELS,
		FlowSharePanelController,
		SHARE_PRINCIPAL_KIND_KEYS,
		shareLevelKey,
		type ShareChange,
		type ShareSnapshot
	} from '#lib/flow/share-panel.js';
	import type { FlowGrantLevel, FlowPrincipalKind } from '#lib/api/flow.js';
	import FlowPublicLinks from './FlowPublicLinks.svelte';

	interface Props {
		objectId: string;
		scopeId: string;
		permittedActions?: readonly string[] | null;
		workspaceId: string | null;
	}

	let { objectId, scopeId, workspaceId, permittedActions = null }: Props = $props();

	let snapshot = $state<ShareSnapshot | null>(null);
	let controller: FlowSharePanelController | null = null;
	let recipientQuery = $state('');
	let directoryError = $state(false);
	let addPrincipal = $state('');
	let addLevel = $state<FlowGrantLevel>('view');
	let liveMessage = $state('');

	function query() {
		return recipientQuery.trim();
	}
	$effect(() => {
		const next = new FlowSharePanelController(
			objectId,
			workspaceId,
			(value) => (snapshot = value),
			workspaceId
				? {}
				: {
						directory: async () => {
							if (query().length < 3) return [];
							const result = await flowApi.listSharePrincipals(scopeId, query());
							directoryError = result.code !== 0;
							return result.data?.items ?? [];
						}
					}
		);
		controller = next;
		addPrincipal = '';
		void next.load();
		return () => {
			if (controller === next) controller = null;
		};
	});

	const permittedToManage = $derived(
		permittedActions === null || permittedActions.includes('manage')
	);
	let lastPermissionKey: string | null = null;
	$effect(() => {
		const key = permittedActions?.join('|') ?? null;
		if (key !== lastPermissionKey) {
			if (lastPermissionKey !== null) void controller?.load();
			lastPermissionKey = key;
		}
	});

	const ready = $derived(snapshot?.load.status === 'ready' ? snapshot.load : null);
	const pending = $derived(snapshot?.pending ?? null);
	const dialogOpen = $derived(
		pending !== null &&
			(pending.phase === 'previewing' ||
				pending.phase === 'confirm' ||
				(pending.phase === 'failed' && pending.preview !== null))
	);
	const busy = $derived(
		pending !== null && (pending.phase === 'previewing' || pending.phase === 'submitting')
	);

	function nameOf(kind: string, id: string): string {
		const entry = snapshot?.directory.find(
			(candidate) => candidate.principal_kind === kind && candidate.principal_id === id
		);
		return entry ? entry.label : `${id.slice(0, 8)}…`;
	}

	function kindLabel(kind: string): string {
		const key = (SHARE_PRINCIPAL_KIND_KEYS as Record<string, string>)[kind];
		return key ? $t(key) : kind;
	}

	const addCandidates = $derived(
		(snapshot?.directory ?? []).filter(
			(entry) =>
				!ready?.view.items.some(
					(item) =>
						item.principal_kind === entry.principal_kind && item.principal_id === entry.principal_id
				)
		)
	);

	const memberCandidates = $derived(
		addCandidates.filter((entry) => entry.principal_kind === 'user')
	);
	const botCandidates = $derived(addCandidates.filter((entry) => entry.principal_kind === 'bot'));

	async function run(change: ShareChange): Promise<void> {
		if (!controller || !permittedToManage) return;
		const outcome = await controller.propose(change);
		if (outcome.status === 'committed') liveMessage = $t('flow.share.saved');
		else if (outcome.status === 'failed') liveMessage = $t(outcome.failure.messageKey);
	}

	async function submitAdd(): Promise<void> {
		const [kind, ...rest] = addPrincipal.split(':');
		const id = rest.join(':');
		if (!id || (kind !== 'user' && kind !== 'bot')) return;
		await run({
			kind: 'add',
			principal_kind: kind as FlowPrincipalKind,
			principal_id: id,
			level: addLevel
		});
		addPrincipal = '';
	}

	async function confirmChange(): Promise<void> {
		if (!controller || !permittedToManage) return;
		const outcome = await controller.confirm();
		if (outcome.status === 'committed') liveMessage = $t('flow.share.saved');
		else if (outcome.status === 'failed') liveMessage = $t(outcome.failure.messageKey);
	}

	function cancelChange(): void {
		controller?.cancel();
	}

	function onAcknowledge(event: Event): void {
		void controller?.acknowledgeSelfLockout((event.currentTarget as HTMLInputElement).checked);
	}

	const selectClass =
		'min-h-11 w-full rounded-control border border-input bg-field px-2 text-sm text-fg';
	const linkButtonClass =
		'min-h-11 rounded-control px-2 text-xs font-medium text-fg underline disabled:text-control disabled:hover:bg-transparent';
</script>

{#if !workspaceId && ready?.canManage && permittedToManage}
	<form
		class="mb-3 space-y-2"
		onsubmit={(event) => {
			event.preventDefault();
			void controller?.load();
		}}
	>
		<label for="flow-share-recipient-query" class="block text-sm"
			>{$t('flow.personal.findUser')}</label
		>
		<input
			id="flow-share-recipient-query"
			class="min-h-11 w-full rounded-control border border-input bg-field px-2"
			bind:value={recipientQuery}
			minlength="3"
			maxlength="256"
			required
		/>
		<button
			type="submit"
			class="min-h-11 px-2 underline"
			disabled={recipientQuery.trim().length < 3}>{$t('flow.personal.search')}</button
		>
		{#if directoryError}<p role="alert" class="text-sm text-danger">
				{$t('flow.personal.recipientSearchFailed')}
			</p>{/if}
	</form>
{/if}

<div class="space-y-4 text-sm" data-testid="flow-share-panel">
	<p class="sr-only" aria-live="polite">{liveMessage}</p>
	{#if !snapshot || snapshot.load.status === 'loading'}
		<p class="text-fg-muted">{$t('flow.share.loading')}</p>
	{:else if snapshot.load.status === 'failed'}
		<p class="text-danger" role="alert">
			{$t(snapshot.load.failure.messageKey)}
		</p>
		<button type="button" class={linkButtonClass} onclick={() => controller?.load()}>
			{$t('flow.share.retry')}
		</button>
	{:else if ready}
		<dl class="space-y-2">
			<div class="flex items-center justify-between gap-2">
				<dt class="text-fg-muted">{$t('flow.share.yourAccess')}</dt>
				<dd
					class="font-medium text-fg"
					data-testid="flow-share-effective"
				>
					{$t(shareLevelKey(ready.view.effective_level))}
				</dd>
			</div>
			<div class="flex items-center justify-between gap-2">
				<dt class="text-fg-muted">{$t('flow.share.inheritance.label')}</dt>
				<dd class="text-fg" data-testid="flow-share-inheritance">
					{ready.view.inherit_from_parent
						? $t('flow.share.inheritance.on')
						: $t('flow.share.inheritance.off')}
				</dd>
			</div>
		</dl>

		{#if !ready.canManage || !permittedToManage}
			<p class="text-fg-muted" data-testid="flow-share-restricted">
				{$t('flow.share.restricted')}
			</p>
		{:else}
			<button
				type="button"
				class="min-h-11 w-full rounded-control border border-fg bg-raised px-3 text-sm font-medium text-fg hover:bg-surface disabled:bg-surface disabled:text-control disabled:border-line"
				disabled={busy}
				onclick={() =>
					run({ kind: 'inheritance', inherit_from_parent: !ready.view.inherit_from_parent })}
			>
				{ready.view.inherit_from_parent
					? $t('flow.share.inheritance.break')
					: $t('flow.share.inheritance.restore')}
			</button>

			<section aria-labelledby="flow-share-inherited">
				<h3
					id="flow-share-inherited"
					class="mb-2 text-xs font-semibold uppercase text-fg-muted"
				>
					{$t('flow.share.inheritedHeading')}
				</h3>
				{#if ready.view.inherited.length === 0}
					<p class="text-fg-muted">{$t('flow.share.inheritedEmpty')}</p>
				{:else}
					<ul class="space-y-1" data-testid="flow-share-inherited">
						{#each ready.view.inherited as grant (`${grant.object_id}:${grant.principal_kind}:${grant.principal_id}`)}
							<li class="flex items-center justify-between gap-2">
								<span class="truncate text-fg">
									{nameOf(grant.principal_kind, grant.principal_id)}
									<span class="text-xs text-fg-muted">· {kindLabel(grant.principal_kind)}</span>
								</span>
								<span class="text-fg-muted"
									>{$t(shareLevelKey(grant.level))}</span
								>
							</li>
						{/each}
					</ul>
				{/if}
			</section>

			<section aria-labelledby="flow-share-explicit">
				<h3
					id="flow-share-explicit"
					class="mb-2 text-xs font-semibold uppercase text-fg-muted"
				>
					{$t('flow.share.explicitHeading')}
				</h3>
				{#if ready.view.items.length === 0}
					<p class="text-fg-muted">{$t('flow.share.explicitEmpty')}</p>
				{:else}
					<ul class="space-y-2" data-testid="flow-share-explicit">
						{#each ready.view.items as grant (`${grant.principal_kind}:${grant.principal_id}`)}
							{@const name = nameOf(grant.principal_kind, grant.principal_id)}
							<li class="flex flex-col gap-1" data-testid="flow-share-grant">
								<span class="truncate text-fg">
									{name}
									<span class="text-xs text-fg-muted">· {kindLabel(grant.principal_kind)}</span>
								</span>
								<div class="flex items-center gap-2">
									<select
										class={selectClass}
										aria-label={$t('flow.share.levelFor', { values: { name } })}
										value={grant.level}
										disabled={busy}
										onchange={(event) =>
											run({
												kind: 'set_level',
												principal_kind: grant.principal_kind,
												principal_id: grant.principal_id,
												level: (event.currentTarget as HTMLSelectElement).value as FlowGrantLevel
											})}
									>
										{#each FLOW_GRANT_LEVELS as level (level)}
											<option value={level}>{$t(shareLevelKey(level))}</option>
										{/each}
									</select>
									<button
										type="button"
										class={linkButtonClass}
										aria-label={$t('flow.share.removeFor', { values: { name } })}
										disabled={busy}
										onclick={() =>
											run({
												kind: 'remove',
												principal_kind: grant.principal_kind,
												principal_id: grant.principal_id
											})}
									>
										{$t('flow.share.remove')}
									</button>
								</div>
							</li>
						{/each}
					</ul>
					<button
						type="button"
						class={linkButtonClass}
						disabled={busy}
						onclick={() => run({ kind: 'clear' })}
					>
						{$t('flow.share.clear')}
					</button>
				{/if}
			</section>

			<section aria-labelledby="flow-share-add" class="space-y-2">
				<h3
					id="flow-share-add"
					class="text-xs font-semibold uppercase text-fg-muted"
				>
					{$t('flow.share.add.heading')}
				</h3>
				<label class="block">
					<span class="mb-1 block text-xs text-fg-muted"
						>{$t('flow.share.add.subject')}</span
					>
					<select
						aria-label={$t('flow.share.add.subject')}
						class={selectClass}
						bind:value={addPrincipal}
						disabled={busy}
						data-testid="flow-share-add-principal"
					>
						<option value="">{$t('flow.share.add.chooseSubject')}</option>
						{#if memberCandidates.length > 0}
							<optgroup label={$t('flow.share.add.members')}>
								{#each memberCandidates as entry (`${entry.principal_kind}:${entry.principal_id}`)}
									<option value={`${entry.principal_kind}:${entry.principal_id}`}
										>{entry.label}</option
									>
								{/each}
							</optgroup>
						{/if}
						{#if botCandidates.length > 0}
							<optgroup label={$t('flow.share.add.bots')}>
								{#each botCandidates as entry (`${entry.principal_kind}:${entry.principal_id}`)}
									<option value={`${entry.principal_kind}:${entry.principal_id}`}
										>{entry.label}</option
									>
								{/each}
							</optgroup>
						{/if}
					</select>
				</label>
				<label class="block">
					<span class="mb-1 block text-xs text-fg-muted"
						>{$t('flow.share.add.level')}</span
					>
					<select
						aria-label={$t('flow.share.add.level')}
						class={selectClass}
						bind:value={addLevel}
						disabled={busy}
					>
						{#each FLOW_GRANT_LEVELS as level (level)}
							<option value={level}>{$t(shareLevelKey(level))}</option>
						{/each}
					</select>
				</label>
				<button
					type="button"
					class="min-h-11 w-full rounded-control bg-primary rf-filled px-3 text-sm font-medium text-on-primary disabled:bg-surface disabled:text-control"
					disabled={busy || addPrincipal === ''}
					onclick={submitAdd}
				>
					{$t('flow.share.add.submit')}
				</button>
			</section>

			{#if pending && pending.phase === 'failed' && pending.preview === null && pending.failure}
				<div class="space-y-1" role="alert">
					<p class="text-danger">{$t(pending.failure.messageKey)}</p>
					{#if pending.failure.retryable}
						<button type="button" class={linkButtonClass} onclick={confirmChange}
							>{$t('flow.share.retry')}</button
						>
					{/if}
					<button type="button" class={linkButtonClass} onclick={cancelChange}
						>{$t('common.cancel')}</button
					>
				</div>
			{/if}

			{#if snapshot.receipt}
				<p class="text-xs text-fg-muted" data-testid="flow-share-receipt">
					{$t('flow.share.receipt', {
						values: { count: snapshot.receipt.changes.affected.length }
					})}
				</p>
			{/if}

			<!-- Creating and revoking a public link is a manage action, same gate as grants. -->
			<FlowPublicLinks {objectId} />
		{/if}
	{/if}
</div>

<Modal open={dialogOpen} title={$t('flow.share.confirm.title')} onclose={cancelChange}>
	{#if pending}
		<div
			class="space-y-3 text-sm text-fg"
			data-testid="flow-share-confirm"
		>
			{#if pending.phase === 'previewing'}
				<p aria-live="polite">{$t('flow.share.confirm.previewing')}</p>
			{/if}
			{#if pending.caller}
				<p>{$t('flow.share.confirm.intro')}</p>
				<p class="font-medium" data-testid="flow-share-confirm-you">
					{$t('flow.share.confirm.you', {
						values: {
							before: $t(shareLevelKey(pending.caller.before_level)),
							after: $t(shareLevelKey(pending.caller.after_level))
						}
					})}
				</p>
			{/if}
			{#if pending.preview}
				{#if pending.preview.affected.length === 0}
					<p>{$t('flow.share.confirm.noneAffected')}</p>
				{:else}
					<ul class="list-disc space-y-1 pl-5" data-testid="flow-share-confirm-affected">
						{#each pending.preview.affected as change (`${change.principal_kind}:${change.principal_id}`)}
							<li>
								{$t('flow.share.confirm.affected', {
									values: {
										name: nameOf(change.principal_kind, change.principal_id),
										before: $t(shareLevelKey(change.before_level)),
										after: $t(shareLevelKey(change.after_level))
									}
								})}
							</li>
						{/each}
					</ul>
				{/if}
			{:else if pending.selfLockout}
				<p>{$t('flow.share.confirm.unknownAffected')}</p>
			{/if}
			{#if pending.selfLockout}
				<p class="font-medium text-danger" role="alert">
					{$t('flow.share.confirm.selfLockout')}
				</p>
				<label
					class="flex min-h-11 cursor-pointer items-center gap-3 text-fg"
				>
					<input
						type="checkbox"
						class="h-5 w-5"
						checked={pending.acknowledged}
						disabled={pending.phase === 'previewing' || pending.phase === 'submitting'}
						onchange={onAcknowledge}
						data-testid="flow-share-confirm-lockout"
					/>
					<span>{$t('flow.share.confirm.acknowledge')}</span>
				</label>
			{/if}
			<p class="text-xs text-fg-muted">
				{$t('flow.share.confirm.previewNote')}
			</p>
			{#if pending.phase === 'failed' && pending.failure}
				<p class="text-danger" role="alert">{$t(pending.failure.messageKey)}</p>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		<button
			type="button"
			class="min-h-11 rounded-control px-4 text-sm font-medium text-fg hover:bg-surface"
			onclick={cancelChange}
		>
			{$t('common.cancel')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded-control bg-danger px-4 text-sm font-medium text-on-danger rf-filled disabled:bg-surface disabled:text-control"
			disabled={!permittedToManage ||
				!pending ||
				pending.phase === 'previewing' ||
				pending.phase === 'submitting' ||
				(pending.phase === 'failed' && !pending.failure?.retryable) ||
				(pending.selfLockout && !pending.acknowledged)}
			onclick={confirmChange}
		>
			{$t('flow.share.confirm.submit')}
		</button>
	{/snippet}
</Modal>
