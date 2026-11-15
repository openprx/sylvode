<script lang="ts">
	import FlowShell from '#lib/components/flow/FlowShell.svelte';
	let { data, children } = $props();
</script>

<FlowShell {data} {children} />
