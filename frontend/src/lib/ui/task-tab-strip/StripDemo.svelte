<script lang="ts">
	import { t } from 'svelte-i18n';
	import TaskTabStrip, { type StripTab } from './TaskTabStrip.svelte';
	import { reorder } from './tab-strip';

	interface Props {
		prefix: string;
		titles: string[];
		/** Positions (0-based) that start unsaved. */
		dirty?: number[];
		changed?: number[];
		loading?: number[];
		width?: number;
		/** Shows the blocking menu for a new item (every tab is unsaved). */
		pendingOpen?: boolean;
	}

	let {
		prefix,
		titles,
		dirty = [],
		changed = [],
		loading = [],
		width,
		pendingOpen = false
	}: Props = $props();

	// The props only seed the demo state.
	// svelte-ignore state_referenced_locally
	let tabs = $state<StripTab[]>(
		titles.map((title, i) => ({
			key: `${prefix}-${i}`,
			title,
			dirty: dirty.includes(i),
			changed: changed.includes(i),
			loading: loading.includes(i)
		}))
	);
	// svelte-ignore state_referenced_locally
	let active = $state<string | null>(`${prefix}-0`);
	let confirming = $state<string | null>(null);
	let log = $state<string[]>([]);

	function close(key: string) {
		const tab = tabs.find((x) => x.key === key);
		if (tab?.dirty) {
			confirming = key;
			return;
		}
		drop(key);
	}
	function drop(key: string) {
		const index = tabs.findIndex((x) => x.key === key);
		tabs = tabs.filter((x) => x.key !== key);
		confirming = null;
		if (active === key) active = tabs[Math.min(index, tabs.length - 1)]?.key ?? null;
	}
</script>

<div class="demo" data-log={log.join('|')} data-active={active} data-count={tabs.length}>
	<TaskTabStrip
		{tabs}
		{active}
		{width}
		{confirming}
		pending={pendingOpen ? { title: $t('ui.taskTabStrip.sampleForm') } : null}
		panelId={(key) => `panel-${key}`}
		onselect={(key) => (active = key)}
		onclose={close}
		onconfirmdiscard={drop}
		oncanceldiscard={() => (confirming = null)}
		onmove={(key, to) =>
			(tabs = reorder(
				tabs.map((x) => x.key),
				key,
				to
			).map((k) => tabs.find((x) => x.key === k)!))}
		onresolvepending={(key) => {
			log.push(key === null ? 'cancel' : `choose:${key}`);
			if (key) confirming = key;
		}}
		oncloseSaved={() => (tabs = tabs.filter((x) => x.dirty))}
		oncloseOthers={(key) => (tabs = tabs.filter((x) => x.key === key || x.dirty))}
	/>
</div>
