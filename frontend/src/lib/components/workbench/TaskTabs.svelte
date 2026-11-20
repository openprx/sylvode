<script lang="ts">
	import { t } from 'svelte-i18n';
	import { onMount, tick } from 'svelte';
	import TaskTabStrip from '#lib/ui/task-tab-strip/TaskTabStrip.svelte';
	import ToastRegion from '#lib/ui/toast/ToastRegion.svelte';
	import { Toasts } from '#lib/ui/toast/toast-store.js';
	import { navigateTask } from '#lib/workbench/navigation.js';
	import type { DroppedTab, TaskWorkbench } from '#lib/workbench/tabs.js';
	let { workbench }: { workbench: TaskWorkbench } = $props();
	let confirming = $state<string | null>(null);
	let origin: HTMLElement | null = null;
	const toasts = new Toasts();

	const label = (title: string) => (title.startsWith('workbench.') ? $t(title) : title);
	const tabs = $derived(
		$workbench.tabs.map((tab) => ({
			key: tab.key,
			title: label(tab.title),
			dirty: tab.dirty,
			changed: tab.changed,
			loading: false
		}))
	);
	const pending = $derived($workbench.pending ? { title: label($workbench.pending.title) } : null);
	const panelId = (key: string) => `task-panel-${encodeURIComponent(key)}`;

	// Remember what had focus when a navigation was refused, to give it back afterwards.
	let hadPending = false;
	$effect.pre(() => {
		const has = $workbench.pending !== null;
		if (has && !hadPending) {
			const element = document.activeElement;
			origin = element instanceof HTMLElement ? element : null;
		}
		hadPending = has;
	});

	function focusOrigin() {
		void tick().then(() => {
			const target =
				origin && origin.isConnected
					? origin
					: document.querySelector<HTMLElement>('[data-testid="task-tab"][aria-selected="true"]');
			target?.focus();
			origin = null;
		});
	}

	function dropped(tab: DroppedTab, next: string) {
		toasts.show({
			message: $t('workbench.droppedTab', {
				values: { title: label(tab.title), next: label(next) }
			}),
			action: { label: $t('workbench.reopen'), run: () => navigateTask(tab.url) }
		});
	}
	onMount(() => {
		workbench.onReplaced = dropped;
		return () => {
			if (workbench.onReplaced === dropped) workbench.onReplaced = null;
		};
	});
	let noticed = false;
	$effect(() => {
		const trimmed = $workbench.restoredTrimmed === true;
		if (trimmed && !noticed) toasts.show({ message: $t('workbench.restoredLimit') });
		noticed = trimmed;
	});

	async function select(key: string) {
		const tab = $workbench.tabs.find((item) => item.key === key);
		if (tab) await navigateTask(tab.url);
	}
	async function close(key: string, discard = false) {
		const result = workbench.close(key, discard);
		if (result.kind === 'confirm') {
			confirming = key;
			return;
		}
		confirming = null;
		if (result.kind === 'closed' && result.wasActive) await navigateTask(result.url);
	}
	async function confirmDiscard(key: string) {
		if ($workbench.pending) {
			const result = workbench.resolvePending(key, true);
			confirming = null;
			if (result.kind === 'closed') await navigateTask(result.href);
			return;
		}
		await close(key, true);
	}
	function resolvePending(key: string | null) {
		if (key === null) {
			workbench.resolvePending(null);
			focusOrigin();
		} else confirming = key;
	}
	async function closeSaved(except?: string) {
		const active = $workbench.active;
		const closed = workbench.closeSaved(except);
		if (active && closed.includes(active)) {
			const next =
				workbench.current.tabs.find((tab) => tab.key === except) ?? workbench.current.tabs[0];
			await navigateTask(next?.url ?? '/workspace');
		}
	}
</script>

<TaskTabStrip
	{tabs}
	active={$workbench.active}
	{pending}
	{panelId}
	onselect={select}
	onclose={(key) => close(key)}
	onconfirmdiscard={confirmDiscard}
	oncanceldiscard={() => (confirming = null)}
	onmove={(key, toIndex) => workbench.moveTab(key, toIndex)}
	onresolvepending={resolvePending}
	oncloseSaved={() => closeSaved()}
	oncloseOthers={(key) => closeSaved(key)}
	{confirming}
/>
<ToastRegion {toasts} />
