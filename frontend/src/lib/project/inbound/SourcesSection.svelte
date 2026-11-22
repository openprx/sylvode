<svelte:options runes />

<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { beforeNavigate, goto } from '$app/navigation';
	import { inboundApi } from '#lib/api/inbound.js';
	import { membersApi } from '#lib/api/members.js';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import SectionFrame from './SectionFrame.svelte';
	import SourceCard from './SourceCard.svelte';
	import SourceEvents from './SourceEvents.svelte';
	import SourceForm from './SourceForm.svelte';
	import SourceSecrets from './SourceSecrets.svelte';
	import { loadStateOf, type LoadState } from './load-state';
	import {
		NO_SECRETS,
		discard,
		finish,
		needsLeaveConfirmation,
		setStored,
		showSecrets,
		type SecretsState
	} from './secrets-state';
	import { replaceSource, withoutSource } from './source-view';
	import type { PersonRef } from './status';
	import type { SecretsAnswer, Source } from './wire';

	interface Props {
		workspaceId: string;
		projectId: string;
	}
	let { workspaceId, projectId }: Props = $props();

	let loadState = $state<LoadState>('loading');
	let inboundEnabled = $state(true);
	let sources = $state<Source[]>([]);
	let members = $state<PersonRef[] | null>(null);
	let now = $state(new Date());
	let creating = $state(false);
	let editingId = $state<string | null>(null);
	let eventsId = $state<string | null>(null);
	let stale = $state(false);

	/**
	 * The one-time credentials. They live here, in this component's state, and nowhere else: not
	 * in a store, storage, the URL or a log. Finishing, agreeing to leave or dropping the component
	 * replaces them with `NO_SECRETS`. Only the source's name and signature rule (neither is a
	 * secret) are kept beside them for the wording.
	 */
	let secrets = $state<SecretsState>(NO_SECRETS);
	let secretsSource = $state<{ name: string; requireSignature: boolean }>({
		name: '',
		requireSignature: true
	});

	/** Reads the list again. Never touches `secrets`: a refresh cannot lose what is on screen. */
	async function load(): Promise<void> {
		const result = await inboundApi.listSources(projectId);
		now = new Date();
		if (result.ok) {
			sources = result.value.sources;
			inboundEnabled = result.value.inbound_enabled;
			stale = false;
			if (editingId !== null && !sources.some((s) => s.id === editingId)) editingId = null;
			if (eventsId !== null && !sources.some((s) => s.id === eventsId)) eventsId = null;
		} else if (loadState !== 'loading') {
			toast.error(result.message);
			return;
		}
		loadState = loadStateOf(result);
	}

	async function loadMembers(): Promise<void> {
		const result = await membersApi.list(workspaceId);
		if (result.code !== 0 || !result.data) return;
		members = result.data.items.map((m) => ({ user_id: m.user_id, name: m.name, role: m.role }));
	}

	onMount(() => {
		void load();
		void loadMembers();
	});

	/** The credentials of a create or rotate answer go into the one-time state; nothing else keeps them. */
	function take(answer: SecretsAnswer): void {
		secrets = showSecrets({
			endpoint: answer.endpoint_url,
			token: answer.token,
			signingSecret: answer.signing_secret
		});
		secretsSource = {
			name: answer.source.name,
			requireSignature: answer.source.require_signature
		};
		creating = false;
		editingId = null;
		eventsId = null;
	}

	function created(answer: SecretsAnswer): void {
		sources = [...sources, answer.source];
		take(answer);
	}

	function rotated(answer: SecretsAnswer): void {
		sources = replaceSource(sources, answer.source);
		take(answer);
	}

	function saved(source: Source): void {
		sources = replaceSource(sources, source);
		editingId = null;
	}

	function deleted(id: string): void {
		sources = withoutSource(sources, id);
		if (eventsId === id) eventsId = null;
	}

	function toggleEvents(id: string): void {
		eventsId = eventsId === id ? null : id;
	}

	// --- leaving with unconfirmed credentials asks ---------------------------------------------
	beforeNavigate((navigation) => {
		if (!needsLeaveConfirmation(secrets)) return;
		navigation.cancel();
		const target = navigation.to?.url ?? null;
		if (navigation.type === 'leave' || target === null) return;
		void confirmAction({
			title: $t('inbound.secrets.leave'),
			confirmLabel: $t('inbound.secrets.leaveConfirm')
		}).then((leave) => {
			if (!leave) return;
			secrets = discard();
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- the target is the URL the person was already going to
			void goto(target);
		});
	});
	$effect(() => {
		const warn = (event: BeforeUnloadEvent) => {
			if (!needsLeaveConfirmation(secrets)) return;
			event.preventDefault();
			event.returnValue = '';
		};
		window.addEventListener('beforeunload', warn);
		return () => window.removeEventListener('beforeunload', warn);
	});
	onDestroy(() => {
		secrets = discard();
	});
</script>

<SectionFrame
	titleKey="inbound.sources.title"
	descriptionKey="inbound.sources.description"
	thingKey="inbound.sources.thing"
	state={loadState}
	{inboundEnabled}
	testId="inbound-sources-section"
	onretry={() => {
		loadState = 'loading';
		void load();
	}}
>
	{#if secrets.phase === 'shown'}
		<SourceSecrets
			{secrets}
			sourceName={secretsSource.name}
			requireSignature={secretsSource.requireSignature}
			onstored={(stored) => (secrets = setStored(secrets, stored))}
			ondone={() => (secrets = finish(secrets))}
		/>
	{:else}
		{#if stale}
			<TipBar tone="warning" action={{ label: $t('inbound.extra.reload'), run: () => void load() }}>
				{$t('inbound.extra.stale')}
			</TipBar>
		{/if}
		{#if creating}
			<SourceForm
				{projectId}
				oncancel={() => (creating = false)}
				oncreated={created}
				onstale={() => (stale = true)}
			/>
		{:else if sources.length === 0}
			<StateBlock
				kind="empty"
				message={$t('inbound.sources.empty')}
				action={{ label: $t('inbound.sources.add'), run: () => (creating = true), primary: true }}
			/>
		{:else}
			<div class="bar">
				<Button variant="primary" onclick={() => (creating = true)}>
					{$t('inbound.sources.add')}
				</Button>
			</div>
			<div class="list">
				{#each sources as source (source.id)}
					<SourceCard
						{source}
						{projectId}
						{members}
						{now}
						editing={editingId === source.id}
						eventsOpen={eventsId === source.id}
						onedit={() => (editingId = source.id)}
						oncanceledit={() => (editingId = null)}
						onsaved={saved}
						onevents={() => toggleEvents(source.id)}
						onrotated={rotated}
						ondeleted={deleted}
						onstale={() => (stale = true)}
					>
						<SourceEvents {source} {workspaceId} {projectId} onclose={() => (eventsId = null)} />
					</SourceCard>
				{/each}
			</div>
		{/if}
	{/if}
</SectionFrame>

<style>
	.bar {
		display: flex;
		justify-content: flex-end;
	}
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-4);
	}
</style>
