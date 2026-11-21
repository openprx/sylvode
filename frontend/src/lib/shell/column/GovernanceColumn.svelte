<script lang="ts">
	import { t } from 'svelte-i18n';
	import { governanceRows, type Translate } from './lists';
	import NavList from './NavList.svelte';
	import { selectedRowId, type NavRow } from './rows';
	import type { RouteUrl } from './types';

	interface Props {
		url: RouteUrl;
		instanceAdmin: boolean;
		onfollow?: (row?: NavRow) => void;
	}
	let { url, instanceAdmin, onfollow }: Props = $props();

	const tr: Translate = (key, values) => $t(key, values ? { values } : undefined);
	const rows = $derived(governanceRows({ instanceAdmin, tr }));
	const selectedId = $derived(selectedRowId(rows, url));
</script>

<NavList {rows} {selectedId} {onfollow} />
