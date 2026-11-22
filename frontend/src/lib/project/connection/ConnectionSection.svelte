<svelte:options runes />

<script lang="ts">
	import { onDestroy, onMount } from 'svelte';
	import { t } from 'svelte-i18n';
	import { beforeNavigate, goto } from '$app/navigation';
	import { botConnectionApi } from '#lib/api/bot-connection.js';
	import { projectsApi } from '#lib/api/projects.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import PageHeader from '#lib/ui/page-header/PageHeader.svelte';
	import StateBlock from '#lib/ui/state-block/StateBlock.svelte';
	import TipBar from '#lib/ui/tip-bar/TipBar.svelte';
	import {
		NO_BLOCK,
		discard,
		finish,
		needsLeaveConfirmation,
		setStored,
		showBlock,
		type BlockState
	} from './block-state';
	import ConfigBlock from './ConfigBlock.svelte';
	import ConnectForm from './ConnectForm.svelte';
	import ConnectedView from './ConnectedView.svelte';
	import { WAITING_POLL_SECONDS, shouldPollWaiting } from './status';
	import Button from '#lib/ui/button/Button.svelte';
	import { anyWaiting, dropItem, putItem } from './list';
	import type { ConnectionItem, WriteAnswer } from './wire';

	interface Props {
		workspaceId: string;
		projectId: string;
	}
	let { workspaceId, projectId }: Props = $props();

	type Load = 'loading' | 'error' | 'forbidden' | 'ready';
	let load = $state<Load>('loading');
	let projectName = $state('');
	let items = $state<ConnectionItem[]>([]);
	let formOpen = $state(false);
	let conflict = $state(false);
	let refreshing = $state(false);
	let now = $state(new Date());
	/**
	 * The one-time configuration block. It lives here, in this component's state, and nowhere
	 * else: not in a store, storage, the URL or a log. Dropping the component, finishing the
	 * block or agreeing to leave replaces it with `NO_BLOCK`.
	 */
	let block = $state<BlockState>(NO_BLOCK);

	/** Reads the connections again. Never touches `block`: a refresh cannot lose what is on screen. */
	async function reload(): Promise<void> {
		refreshing = true;
		const result = await botConnectionApi.list(projectId);
		refreshing = false;
		now = new Date();
		if (!result.ok) {
			if (load === 'loading') load = 'error';
			else toast.error(result.message);
			return;
		}
		if (result.value.kind === 'reduced') {
			load = 'forbidden';
			return;
		}
		conflict = false;
		items = result.value.items;
		load = 'ready';
	}

	onMount(async () => {
		const project = await projectsApi.get(projectId);
		projectName = project.data?.name ?? '';
		await reload();
	});

	function take(answer: WriteAnswer) {
		items = putItem(items, { connection: answer.connection, status: answer.status });
		conflict = false;
		if (answer.block) block = showBlock(answer.block.text);
	}

	function connected(answer: WriteAnswer) {
		take(answer);
		formOpen = false;
		if (!answer.block)
			toast.success(
				$t('execution.connection.connectedNoChange', {
					values: { directory: answer.connection.execution.working_directory }
				})
			);
	}

	function removed(botId: string) {
		items = dropItem(items, botId);
		conflict = false;
	}

	// --- the status asks again while the runner has not shown up -------------------------------
	let waitedSeconds = 0;
	/** The cards ask again while any of them waits for its runner. */
	const shouldPoll = (waited: number): boolean =>
		items.some((entry) => shouldPollWaiting(entry.status, waited));
	$effect(() => {
		if (!shouldPoll(waitedSeconds)) return;
		const timer = setInterval(() => {
			waitedSeconds += WAITING_POLL_SECONDS;
			if (!shouldPoll(waitedSeconds)) {
				clearInterval(timer);
				return;
			}
			void reload();
		}, WAITING_POLL_SECONDS * 1000);
		return () => clearInterval(timer);
	});
	$effect(() => {
		if (!anyWaiting(items)) waitedSeconds = 0;
	});

	// --- the section asks again when the window gets focus -------------------------------------
	$effect(() => {
		const onFocus = () => void reload();
		window.addEventListener('focus', onFocus);
		return () => window.removeEventListener('focus', onFocus);
	});

	// --- leaving with an unconfirmed block asks -------------------------------------------------
	beforeNavigate((navigation) => {
		if (!needsLeaveConfirmation(block)) return;
		navigation.cancel();
		const target = navigation.to?.url ?? null;
		if (navigation.type === 'leave' || target === null) return;
		void confirmAction({
			title: $t('execution.connection.block.leave'),
			confirmLabel: $t('execution.connection.block.leaveConfirm')
		}).then((leave) => {
			if (!leave) return;
			block = discard();
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- the target is the URL the person was already going to
			void goto(target);
		});
	});
	$effect(() => {
		const warn = (event: BeforeUnloadEvent) => {
			if (!needsLeaveConfirmation(block)) return;
			event.preventDefault();
			event.returnValue = '';
		};
		window.addEventListener('beforeunload', warn);
		return () => window.removeEventListener('beforeunload', warn);
	});
	onDestroy(() => {
		block = discard();
	});
</script>

<div class="section" data-testid="execution-section">
	<PageHeader title={$t('execution.connection.title')} />
	<p class="description">{$t('execution.connection.description')}</p>

	{#if load === 'loading'}
		<StateBlock kind="loading" thing={$t('execution.connection.loadingThing')} />
	{:else if load === 'error'}
		<StateBlock
			kind="loadFailed"
			thing={$t('execution.connection.loadingThing')}
			action={{ run: () => void reload() }}
		/>
	{:else if load === 'forbidden'}
		<StateBlock kind="forbidden" message={$t('execution.connection.forbidden')} />
	{:else if block.phase === 'shown'}
		<ConfigBlock
			{block}
			onstored={(stored) => (block = setStored(block, stored))}
			ondone={() => (block = finish(block))}
		/>
	{:else}
		{#if conflict}
			<TipBar
				tone="warning"
				action={{ label: $t('execution.connection.reload'), run: () => void reload() }}
			>
				{$t('execution.connection.conflict')}
			</TipBar>
		{/if}
		{#if items.length === 0 && !formOpen}
			<StateBlock
				kind="empty"
				message={$t('execution.connection.empty')}
				action={{
					label: $t('execution.connection.connect'),
					run: () => (formOpen = true),
					primary: true
				}}
			/>
		{/if}
		{#each items as item (item.connection.bot.id)}
			<ConnectedView
				{workspaceId}
				{projectName}
				connection={item.connection}
				connections={items.map((entry) => entry.connection)}
				status={item.status}
				{now}
				{refreshing}
				onrefresh={() => void reload()}
				onchanged={take}
				onremoved={removed}
				onconflict={() => (conflict = true)}
			/>
		{/each}
		{#if formOpen}
			<ConnectForm
				{workspaceId}
				{projectId}
				connections={items.map((entry) => entry.connection)}
				oncancel={() => (formOpen = false)}
				onconnected={connected}
				onreload={() => void reload()}
			/>
		{:else if items.length > 0}
			<div>
				<Button variant="secondary" onclick={() => (formOpen = true)} data-testid="connect-bot">
					{$t('execution.connection.connect')}
				</Button>
			</div>
		{/if}
	{/if}
</div>

<style>
	.section {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 800px;
	}
	.description {
		margin: 0;
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
</style>
