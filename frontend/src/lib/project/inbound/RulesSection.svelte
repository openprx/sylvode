<svelte:options runes />

<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import { t } from 'svelte-i18n';
	import { goto } from '$app/navigation';
	import { botConnectionApi } from '#lib/api/bot-connection.js';
	import { inboundApi } from '#lib/api/inbound.js';
	import { labelsApi } from '#lib/api/labels.js';
	import { membersApi } from '#lib/api/members.js';
	import { authStore } from '#lib/stores/auth.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import DestructiveDialog from '#lib/ui/dialog/DestructiveDialog.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import type { Connection } from '#lib/project/connection/wire.js';
	import SectionFrame from './SectionFrame.svelte';
	import RuleEditor, { type BotOption, type PersonOption } from './RuleEditor.svelte';
	import RuleTestDialog from './RuleTestDialog.svelte';
	import RuleRow from './RuleRow.svelte';
	import { loadStateOf, type LoadState } from './load-state';
	import { movedOrder, toggleBody } from './rule-form';
	import { botNotConnected, hasNoRecipients } from './status';
	import type { Rule, Source } from './wire';

	interface Props {
		workspaceId: string;
		projectId: string;
	}
	let { workspaceId, projectId }: Props = $props();

	let loadState = $state<LoadState>('loading');
	let inboundEnabled = $state(true);
	let sources = $state<Source[]>([]);
	let rules = $state<Rule[]>([]);
	let people = $state<PersonOption[]>([]);
	let bots = $state<BotOption[]>([]);
	let labels = $state<{ id: string; name: string }[]>([]);
	let connection = $state<Connection | null>(null);
	let connectionKnown = $state(false);

	/** `null`: the list; `{ rule: null }`: a new rule; `{ rule }`: that rule's editor. */
	let editing = $state<{ rule: Rule | null } | null>(null);
	let testOpen = $state(false);
	let busy = $state(false);
	let conflict = $state(false);
	let deleting = $state<Rule | null>(null);
	let deleteBusy = $state(false);

	const currentUserId = get(authStore).user?.id ?? null;

	const humanIds = $derived(new Set(people.map((p) => p.id)));
	const sourceName = (id: string | null): string | null =>
		id === null ? null : (sources.find((s) => s.id === id)?.name ?? null);
	const botName = (id: string | null): string | null =>
		id === null ? null : (bots.find((b) => b.id === id)?.name ?? null);
	const connectedBotId = $derived(
		connection !== null && connection.enabled ? connection.bot.id : null
	);

	async function load(): Promise<void> {
		const [sourceResult, ruleResult, memberResult, labelResult, connectionResult] =
			await Promise.all([
				inboundApi.listSources(projectId),
				inboundApi.listRules(projectId),
				membersApi.list(workspaceId),
				labelsApi.list(workspaceId),
				botConnectionApi.get(projectId)
			]);
		if (sourceResult.ok) inboundEnabled = sourceResult.value.inbound_enabled;
		const state = loadStateOf(sourceResult);
		const ruleState = loadStateOf(ruleResult);
		if (state === 'forbidden' || ruleState === 'forbidden') {
			loadState = 'forbidden';
			return;
		}
		if (
			!sourceResult.ok ||
			!ruleResult.ok ||
			memberResult.code !== 0 ||
			!Array.isArray(memberResult.data?.items) ||
			labelResult.code !== 0 ||
			!Array.isArray(labelResult.data?.items)
		) {
			loadState = 'error';
			return;
		}
		sources = sourceResult.value.sources;
		rules = ruleResult.value;
		const members = memberResult.data.items;
		people = members
			.filter((m) => m.entity_type !== 'bot')
			.map((m) => ({ id: m.user_id, name: m.name }));
		bots = members
			.filter((m) => m.entity_type === 'bot')
			.map((m) => ({ id: m.user_id, name: m.name }));
		labels = labelResult.data.items.map((l) => ({ id: l.id, name: l.name }));
		connectionKnown = connectionResult.ok && connectionResult.value.kind !== 'reduced';
		connection =
			connectionResult.ok && connectionResult.value.kind === 'full'
				? connectionResult.value.connection
				: null;
		loadState = 'ready';
	}

	/** Reads the rules again (positions change on create and delete); the list stays on a failure. */
	async function reloadRules(): Promise<void> {
		const result = await inboundApi.listRules(projectId);
		if (result.ok) rules = result.value;
		else toast.error(result.message);
	}

	async function reload(): Promise<void> {
		conflict = false;
		editing = null;
		await reloadRules();
	}

	onMount(() => void load());

	// --- row actions ---------------------------------------------------------------------------

	/** The enable switch writes the WHOLE rule: a replace drops every field it leaves out. */
	async function toggle(rule: Rule, enabled: boolean): Promise<boolean> {
		busy = true;
		conflict = false;
		const result = await inboundApi.replaceRule(rule.id, toggleBody(rule, enabled));
		busy = false;
		if (result.ok) {
			rules = rules.map((r) => (r.id === rule.id ? result.value.rule : r));
			return true;
		}
		if (result.kind === 'conflict') conflict = true;
		else toast.error(result.message);
		return false;
	}

	async function move(rule: Rule, delta: -1 | 1): Promise<void> {
		const next = movedOrder(
			rules.map((r) => r.id),
			rule.id,
			delta
		);
		if (next === null) return;
		busy = true;
		const result = await inboundApi.reorderRules(projectId, next);
		busy = false;
		if (result.ok) rules = result.value;
		else toast.error(result.message);
	}

	async function confirmDelete(): Promise<void> {
		const rule = deleting;
		if (rule === null) return;
		deleteBusy = true;
		const result = await inboundApi.deleteRule(rule.id);
		deleteBusy = false;
		if (!result.ok) {
			toast.error(result.message);
			return;
		}
		deleting = null;
		rules = rules.filter((r) => r.id !== rule.id);
		await reloadRules();
	}

	async function saved(): Promise<void> {
		editing = null;
		toast.success($t('inbound.rules.saved'));
		await reloadRules();
	}

	function toSources(): void {
		const target = `/workspace/${encodeURIComponent(workspaceId)}/projects/${encodeURIComponent(projectId)}/settings/inbound-sources`;
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- a project-relative settings link built like the nav entries
		void goto(target);
	}
</script>

<SectionFrame
	titleKey="inbound.rules.title"
	descriptionKey="inbound.rules.description"
	thingKey="inbound.rules.thing"
	state={loadState}
	{inboundEnabled}
	testId="alert-rules-section"
	onretry={() => {
		loadState = 'loading';
		void load();
	}}
>
	{#if editing !== null}
		{#key editing.rule?.id ?? 'new'}
			<RuleEditor
				{projectId}
				rule={editing.rule}
				{sources}
				{bots}
				{people}
				{labels}
				{currentUserId}
				{connectedBotId}
				{connectionKnown}
				onsaved={() => void saved()}
				oncancel={() => (editing = null)}
				onreload={() => void reload()}
			/>
		{/key}
	{:else if sources.length === 0 && rules.length === 0}
		<StateBlock
			kind="empty"
			thing={$t('inbound.rules.thing')}
			message={$t('inbound.rules.emptyNoSource')}
			action={{ label: $t('inbound.rules.addSource'), run: toSources, primary: true }}
		/>
	{:else}
		{#if conflict}
			<TipBar
				tone="warning"
				action={{ label: $t('inbound.rules.reload'), run: () => void reload() }}
			>
				{$t('inbound.rules.conflict')}
			</TipBar>
		{/if}
		{#if rules.length === 0}
			<StateBlock
				kind="empty"
				thing={$t('inbound.rules.thing')}
				message={$t('inbound.rules.empty')}
				action={{
					label: $t('inbound.rules.add'),
					run: () => (editing = { rule: null }),
					primary: true
				}}
			/>
		{:else}
			<div class="strip">
				<p class="order-help">{$t('inbound.rules.orderHelp')}</p>
				<div class="strip-actions">
					<Button
						variant="secondary"
						disabled={sources.length === 0}
						disabledReason={$t('inbound.test.noSource')}
						onclick={() => (testOpen = true)}
					>
						{$t('inbound.rules.test')}
					</Button>
					<Button variant="primary" onclick={() => (editing = { rule: null })}>
						{$t('inbound.rules.add')}
					</Button>
				</div>
			</div>
			<ol class="list" data-testid="rule-list">
				{#each rules as rule, index (rule.id)}
					<RuleRow
						{rule}
						{index}
						count={rules.length}
						sourceName={sourceName(rule.source_id)}
						botName={botName(rule.bot_user_id)}
						warnBotNotConnected={connectionKnown && botNotConnected(rule, connection)}
						warnNoRecipients={hasNoRecipients(rule, humanIds)}
						{busy}
						ontoggle={(enabled) => toggle(rule, enabled)}
						onmove={(delta) => void move(rule, delta)}
						onedit={() => (editing = { rule })}
						ondelete={() => (deleting = rule)}
					/>
				{/each}
			</ol>
		{/if}
	{/if}
</SectionFrame>

{#if testOpen}
	<RuleTestDialog open onclose={() => (testOpen = false)} {projectId} />
{/if}

<DestructiveDialog
	open={deleting !== null}
	title={$t('inbound.rules.deleteDialog.title', { values: { name: deleting?.name ?? '' } })}
	confirmLabel={$t('inbound.rules.deleteDialog.confirm')}
	busyLabel={$t('inbound.rules.deleteDialog.busy')}
	busy={deleteBusy}
	onconfirm={() => void confirmDelete()}
	onclose={() => {
		if (!deleteBusy) deleting = null;
	}}
	testId="rule-delete-dialog"
>
	{$t('inbound.rules.deleteDialog.body')}
</DestructiveDialog>

<style>
	.strip {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--rf-space-4);
		flex-wrap: wrap;
	}
	.order-help {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.strip-actions {
		display: flex;
		gap: var(--rf-space-3);
	}
	.list {
		margin: 0;
		padding: 0;
		border: var(--rf-border-width) solid var(--rf-line);
		border-radius: var(--rf-radius-card);
		background: var(--rf-surface-white);
	}
</style>
