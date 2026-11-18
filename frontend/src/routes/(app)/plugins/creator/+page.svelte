<script lang="ts">
	import { page } from '$app/state';
	import PluginCreator from '#lib/components/plugins/PluginCreator.svelte';
</script>

<PluginCreator
	context={{ kind: 'personal', draftId: page.url.searchParams.get('draft') ?? 'default' }}
	initialSlot={page.url.searchParams.get('slot')}
	formKey={page.url.searchParams.get('form') ?? 'order_line'}
/>
