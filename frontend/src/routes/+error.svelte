<script lang="ts">
	import { page } from '$app/state';
	import { t } from 'svelte-i18n';
	import RouteError from '#lib/components/RouteError.svelte';
</script>

<svelte:head><title>{$t('errorPage.title')} - Sylvode</title></svelte:head>

<main class="min-h-screen bg-surface px-4 py-12">
	<RouteError status={page.status} message={page.error?.message} />
</main>
