<svelte:options runes />

<script lang="ts">
	import type { Snippet } from 'svelte';
	import { locale, t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { inboundApi, type UpdateSourceInput } from '#lib/api/inbound.js';
	import { absoluteTime, relativeTime } from '#lib/project/agent-tasks/time.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { plural } from '#lib/ui/foundations/plural.js';
	import SettingRow from '#lib/ui/setting-row/SettingRow.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import SourceDialogs, { type PendingAction } from './SourceDialogs.svelte';
	import SourceForm from './SourceForm.svelte';
	import { routeFailure } from './source-form';
	import { boundRules } from './source-view';
	import { actingUserName, sourceChip, sourceStatus, type PersonRef } from './status';
	import type { Rule, SecretsAnswer, Source } from './wire';

	interface Props {
		source: Source;
		projectId: string;
		members: PersonRef[] | null;
		now: Date;
		editing: boolean;
		eventsOpen: boolean;
		onedit: () => void;
		oncanceledit: () => void;
		onsaved: (source: Source) => void;
		onevents: () => void;
		/** A rotation: the answer holds the new credentials, the section's from here on. */
		onrotated: (answer: SecretsAnswer) => void;
		ondeleted: (id: string) => void;
		/** A 409: the source changed or is gone; the section offers to read the list again. */
		onstale: () => void;
		/** The recent events, rendered below the rows while open. */
		children?: Snippet;
	}
	let {
		source,
		projectId,
		members,
		now,
		editing,
		eventsOpen,
		onedit,
		oncanceledit,
		onsaved,
		onevents,
		onrotated,
		ondeleted,
		onstale,
		children
	}: Props = $props();

	let pending = $state<PendingAction>(null);
	let busy = $state(false);
	let askingDelete = $state(false);
	let bound = $state<Rule[]>([]);

	const status = $derived(sourceStatus(source, now, members));
	const statusChip = $derived(sourceChip(status));
	const actsAs = $derived(actingUserName(source, members));
	const blocks = $derived(source.ip_allowlist.length);

	function failed(result: { code: number; message: string; messageKey?: string }): void {
		const routed = routeFailure({ code: result.code, message: describeResult(result) });
		if (routed.type === 'stale') onstale();
		else toast.error(routed.message);
	}

	async function change(input: UpdateSourceInput, done?: string): Promise<boolean> {
		busy = true;
		const result = await inboundApi.updateSource(source.id, input);
		busy = false;
		if (!result.ok) {
			failed(result);
			return false;
		}
		onsaved(result.value);
		if (done) toast.success(done);
		return true;
	}

	async function confirmed(action: Exclude<PendingAction, null>): Promise<void> {
		if (action === 'pause') await change({ enabled: false }, $t('inbound.extra.paused'));
		else if (action === 'takeOver')
			await change({ take_over: true }, $t('inbound.extra.takenOver'));
		else if (action === 'rotate') {
			busy = true;
			const result = await inboundApi.rotateSource(source.id);
			busy = false;
			if (result.ok) onrotated(result.value);
			else failed(result);
		} else {
			busy = true;
			const result = await inboundApi.deleteSource(source.id);
			busy = false;
			if (result.ok) {
				toast.success($t('inbound.extra.deleted'));
				ondeleted(source.id);
			} else failed(result);
		}
		pending = null;
	}

	/** The delete dialog names the rules the cascade takes along, so they are read fresh first. */
	async function askDelete(): Promise<void> {
		askingDelete = true;
		const result = await inboundApi.listRules(projectId);
		askingDelete = false;
		if (!result.ok) {
			toast.error($t('inbound.extra.rulesLoadFailed'));
			return;
		}
		bound = boundRules(result.value, source.id);
		pending = 'delete';
	}

	async function resume(): Promise<void> {
		await change({ enabled: true }, $t('inbound.sources.resumed'));
	}
</script>

<article class="card" data-testid="source-card" data-source-id={source.id}>
	<header class="head">
		<strong class="name">{source.name}</strong>
		<StatusChip tone={statusChip.tone} data-status={status}>{$t(statusChip.key)}</StatusChip>
	</header>

	{#if editing}
		<SourceForm
			{source}
			{projectId}
			oncancel={oncanceledit}
			onsaved={(next) => {
				onsaved(next);
				toast.success($t('inbound.extra.saved'));
			}}
			{onstale}
		/>
	{:else}
		{#if status === 'suspended'}
			<TipBar
				tone="danger"
				action={{ label: $t('inbound.sources.takeOver'), run: () => (pending = 'takeOver') }}
			>
				{$t('inbound.sources.suspendedBody', {
					values: { name: actsAs ?? $t('inbound.sources.row.removedUser') }
				})}
			</TipBar>
		{/if}

		<section class="rows">
			<SettingRow
				label={$t('inbound.sources.row.token')}
				value={`${source.token_prefix}…`}
				data-testid="row-token"
			/>
			<SettingRow
				label={$t('inbound.sources.row.signature')}
				value={source.require_signature
					? $t('inbound.sources.row.signatureRequired')
					: $t('inbound.sources.row.signatureOptional')}
			/>
			<SettingRow
				label={$t('inbound.sources.row.lastEvent')}
				value={source.last_used_at === null
					? $t('inbound.sources.row.never')
					: source.last_used_ip
						? $t('inbound.sources.row.lastEventValue', {
								values: { time: relativeTime(source.last_used_at), ip: source.last_used_ip }
							})
						: relativeTime(source.last_used_at)}
				title={source.last_used_at === null ? undefined : absoluteTime(source.last_used_at)}
			/>
			<SettingRow
				label={$t('inbound.sources.row.actsAs')}
				value={actsAs ?? $t('inbound.sources.row.removedUser')}
			>
				{#snippet chip()}
					<span class="helper">{$t('inbound.sources.row.actsAsHelp')}</span>
				{/snippet}
			</SettingRow>
			<SettingRow
				label={$t('inbound.sources.row.limits')}
				value={$t('inbound.sources.row.limitsValue', {
					values: { rate: source.rate_per_minute, cap: source.daily_cap }
				})}
			/>
			<SettingRow
				label={$t('inbound.sources.row.allowlist')}
				value={blocks === 0 ? $t('inbound.sources.row.anyAddress') : undefined}
			>
				{#snippet chip()}
					{#if blocks > 0}
						<details class="blocks">
							<summary>{plural($t, 'inbound.sources.row.blocks', blocks, $locale)}</summary>
							<ul>
								{#each source.ip_allowlist as entry (entry)}
									<li>{entry}</li>
								{/each}
							</ul>
						</details>
					{/if}
				{/snippet}
			</SettingRow>
		</section>
		{#if status === 'waiting'}
			<p class="hint" data-testid="source-lost-hint">{$t('inbound.secrets.lost')}</p>
		{/if}

		<footer class="actions">
			<Button variant="secondary" aria-expanded={eventsOpen} onclick={onevents}>
				{$t('inbound.sources.recentEvents')}
			</Button>
			<Button variant="secondary" disabled={busy} onclick={onedit}>
				{$t('inbound.sources.edit')}
			</Button>
			{#if source.enabled}
				<Button variant="secondary" disabled={busy} onclick={() => (pending = 'pause')}>
					{$t('inbound.sources.pause')}
				</Button>
			{:else}
				<Button variant="secondary" loading={busy} onclick={() => void resume()}>
					{$t('inbound.sources.resume')}
				</Button>
			{/if}
			<Button variant="secondary" disabled={busy} onclick={() => (pending = 'rotate')}>
				{$t('inbound.sources.rotate')}
			</Button>
			<Button
				variant="danger-quiet"
				loading={askingDelete}
				disabled={busy}
				onclick={() => void askDelete()}
			>
				{$t('inbound.sources.delete')}
			</Button>
		</footer>

		{#if eventsOpen}{@render children?.()}{/if}
	{/if}

	<SourceDialogs
		{source}
		{pending}
		{bound}
		{busy}
		onconfirm={(action) => void confirmed(action)}
		onclose={() => {
			if (!busy) pending = null;
		}}
	/>
</article>

<style>
	.card {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-3);
		padding: var(--rf-space-5);
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
	}
	.head {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--rf-space-4);
	}
	.name {
		font-size: var(--rf-size-body);
		font-weight: 600;
		overflow-wrap: anywhere;
	}
	.rows {
		display: flex;
		flex-direction: column;
	}
	.helper {
		color: var(--rf-secondary);
		font-size: var(--rf-size-meta);
	}
	.hint {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.blocks summary {
		cursor: pointer;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.blocks ul {
		margin: var(--rf-space-2) 0 0;
		padding-left: var(--rf-space-5);
		font-family: var(--rf-font-mono-stack);
		font-size: var(--rf-size-secondary);
		overflow-wrap: anywhere;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
	}
</style>
