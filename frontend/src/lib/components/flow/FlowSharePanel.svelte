<script lang="ts">
	// Share panel (`ui-surface-v1.md` "职责边界" Share panel). All state and every rule lives in
	// `FlowSharePanelController` (`lib/flow/share-panel.ts`); this component only renders it and
	// forwards user actions.
	import { t } from 'svelte-i18n';
	import Modal from '$lib/components/Modal.svelte';
	import {
		FLOW_GRANT_LEVELS,
		FlowSharePanelController,
		SHARE_PRINCIPAL_KIND_KEYS,
		shareLevelKey,
		type ShareChange,
		type ShareSnapshot
	} from '$lib/flow/share-panel';
	import type { FlowGrantLevel, FlowPrincipalKind } from '$lib/api/flow';

	interface Props {
		objectId: string;
		workspaceId: string;
	}

	let { objectId, workspaceId }: Props = $props();

	let snapshot = $state<ShareSnapshot | null>(null);
	let controller: FlowSharePanelController | null = null;
	let addPrincipal = $state('');
	let addLevel = $state<FlowGrantLevel>('view');
	let liveMessage = $state('');

	$effect(() => {
		const next = new FlowSharePanelController(objectId, workspaceId, (value) => (snapshot = value));
		controller = next;
		addPrincipal = '';
		void next.load();
		return () => {
			if (controller === next) controller = null;
		};
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

	async function run(change: ShareChange): Promise<void> {
		if (!controller) return;
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
		if (!controller) return;
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
		'min-h-11 w-full rounded-md border border-slate-300 bg-white px-2 text-sm text-slate-900 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 dark:border-slate-600 dark:bg-slate-800 dark:text-slate-100';
	const linkButtonClass =
		'min-h-11 rounded-md px-2 text-xs font-medium text-blue-600 hover:underline focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:text-blue-400';
</script>

<div class="space-y-4 text-sm" data-testid="flow-share-panel">
	<p class="sr-only" aria-live="polite">{liveMessage}</p>
	{#if !snapshot || snapshot.load.status === 'loading'}
		<p class="text-slate-500 dark:text-slate-400">{$t('flow.share.loading')}</p>
	{:else if snapshot.load.status === 'failed'}
		<p class="text-red-600 dark:text-red-400" role="alert">
			{$t(snapshot.load.failure.messageKey)}
		</p>
		<button type="button" class={linkButtonClass} onclick={() => controller?.load()}>
			{$t('flow.share.retry')}
		</button>
	{:else if ready}
		<dl class="space-y-2">
			<div class="flex items-center justify-between gap-2">
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.share.yourAccess')}</dt>
				<dd
					class="font-medium text-slate-900 dark:text-slate-100"
					data-testid="flow-share-effective"
				>
					{$t(shareLevelKey(ready.view.effective_level))}
				</dd>
			</div>
			<div class="flex items-center justify-between gap-2">
				<dt class="text-slate-500 dark:text-slate-400">{$t('flow.share.inheritance.label')}</dt>
				<dd class="text-slate-700 dark:text-slate-200" data-testid="flow-share-inheritance">
					{ready.view.inherit_from_parent
						? $t('flow.share.inheritance.on')
						: $t('flow.share.inheritance.off')}
				</dd>
			</div>
		</dl>

		{#if !ready.canManage}
			<p class="text-slate-500 dark:text-slate-400" data-testid="flow-share-restricted">
				{$t('flow.share.restricted')}
			</p>
		{:else}
			<button
				type="button"
				class="min-h-11 w-full rounded-md border border-slate-300 px-3 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 disabled:opacity-50 dark:border-slate-600 dark:text-slate-200 dark:hover:bg-slate-800"
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
					class="mb-2 text-xs font-semibold uppercase text-slate-500 dark:text-slate-400"
				>
					{$t('flow.share.inheritedHeading')}
				</h3>
				{#if ready.view.inherited.length === 0}
					<p class="text-slate-500 dark:text-slate-400">{$t('flow.share.inheritedEmpty')}</p>
				{:else}
					<ul class="space-y-1" data-testid="flow-share-inherited">
						{#each ready.view.inherited as grant (`${grant.object_id}:${grant.principal_kind}:${grant.principal_id}`)}
							<li class="flex items-center justify-between gap-2">
								<span class="truncate text-slate-700 dark:text-slate-200">
									{nameOf(grant.principal_kind, grant.principal_id)}
									<span class="text-xs text-slate-400">· {kindLabel(grant.principal_kind)}</span>
								</span>
								<span class="text-slate-500 dark:text-slate-400"
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
					class="mb-2 text-xs font-semibold uppercase text-slate-500 dark:text-slate-400"
				>
					{$t('flow.share.explicitHeading')}
				</h3>
				{#if ready.view.items.length === 0}
					<p class="text-slate-500 dark:text-slate-400">{$t('flow.share.explicitEmpty')}</p>
				{:else}
					<ul class="space-y-2" data-testid="flow-share-explicit">
						{#each ready.view.items as grant (`${grant.principal_kind}:${grant.principal_id}`)}
							{@const name = nameOf(grant.principal_kind, grant.principal_id)}
							<li class="flex flex-col gap-1" data-testid="flow-share-grant">
								<span class="truncate text-slate-700 dark:text-slate-200">
									{name}
									<span class="text-xs text-slate-400">· {kindLabel(grant.principal_kind)}</span>
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
					class="text-xs font-semibold uppercase text-slate-500 dark:text-slate-400"
				>
					{$t('flow.share.add.heading')}
				</h3>
				<label class="block">
					<span class="mb-1 block text-xs text-slate-500 dark:text-slate-400"
						>{$t('flow.share.add.person')}</span
					>
					<select
						aria-label={$t('flow.share.add.person')}
						class={selectClass}
						bind:value={addPrincipal}
						disabled={busy}
					>
						<option value="">{$t('flow.share.add.choose')}</option>
						{#each addCandidates as entry (`${entry.principal_kind}:${entry.principal_id}`)}
							<option value={`${entry.principal_kind}:${entry.principal_id}`}>{entry.label}</option>
						{/each}
					</select>
				</label>
				<label class="block">
					<span class="mb-1 block text-xs text-slate-500 dark:text-slate-400"
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
					class="min-h-11 w-full rounded-md bg-blue-600 px-3 text-sm font-medium text-white hover:bg-blue-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:opacity-50"
					disabled={busy || addPrincipal === ''}
					onclick={submitAdd}
				>
					{$t('flow.share.add.submit')}
				</button>
			</section>

			{#if pending && pending.phase === 'failed' && pending.preview === null && pending.failure}
				<div class="space-y-1" role="alert">
					<p class="text-red-600 dark:text-red-400">{$t(pending.failure.messageKey)}</p>
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
				<p class="text-xs text-slate-500 dark:text-slate-400" data-testid="flow-share-receipt">
					{$t('flow.share.receipt', {
						values: { count: snapshot.receipt.changes.affected.length }
					})}
				</p>
			{/if}
		{/if}
	{/if}
</div>

<Modal open={dialogOpen} title={$t('flow.share.confirm.title')} onclose={cancelChange}>
	{#if pending}
		<div
			class="space-y-3 text-sm text-slate-700 dark:text-slate-300"
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
				<p class="font-medium text-red-600 dark:text-red-400" role="alert">
					{$t('flow.share.confirm.selfLockout')}
				</p>
				<label
					class="flex min-h-11 cursor-pointer items-center gap-3 text-slate-900 dark:text-slate-100"
				>
					<input
						type="checkbox"
						class="h-5 w-5 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500"
						checked={pending.acknowledged}
						disabled={pending.phase === 'previewing' || pending.phase === 'submitting'}
						onchange={onAcknowledge}
						data-testid="flow-share-confirm-lockout"
					/>
					<span>{$t('flow.share.confirm.acknowledge')}</span>
				</label>
			{/if}
			<p class="text-xs text-slate-500 dark:text-slate-400">
				{$t('flow.share.confirm.previewNote')}
			</p>
			{#if pending.phase === 'failed' && pending.failure}
				<p class="text-red-600 dark:text-red-400" role="alert">{$t(pending.failure.messageKey)}</p>
			{/if}
		</div>
	{/if}
	{#snippet footer()}
		<button
			type="button"
			class="min-h-11 rounded-md px-4 text-sm font-medium text-slate-700 hover:bg-slate-100 focus:outline-none focus-visible:ring-2 focus-visible:ring-slate-500 dark:text-slate-200 dark:hover:bg-slate-700"
			onclick={cancelChange}
		>
			{$t('common.cancel')}
		</button>
		<button
			type="button"
			class="min-h-11 rounded-md bg-red-600 px-4 text-sm font-medium text-white hover:bg-red-700 focus:outline-none focus-visible:ring-2 focus-visible:ring-red-500 focus-visible:ring-offset-2 disabled:opacity-50 dark:focus-visible:ring-offset-slate-800"
			disabled={!pending ||
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
