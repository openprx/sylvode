<script lang="ts">
	import { appToasts } from '#lib/ui/toast/app-toasts.js';
	import ToastRegion from '#lib/ui/toast/ToastRegion.svelte';
</script>

<ToastRegion toasts={appToasts} />
