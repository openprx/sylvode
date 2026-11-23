<script lang="ts">
	import { page } from '$app/state';
	import RouteError from '#lib/components/RouteError.svelte';
</script>

<RouteError status={page.status} message={page.error?.message} />
