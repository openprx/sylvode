<script lang="ts">
	import { page } from '$app/state';
	import PluginCreator from '#lib/components/plugins/PluginCreator.svelte';
</script>

<PluginCreator
	context={{
		kind: 'project',
		workspaceId: page.params.workspaceId ?? '',
		projectId: page.params.projectId ?? ''
	}}
	initialSlot={page.url.searchParams.get('slot')}
	formKey={page.url.searchParams.get('form')}
	initialTemplateId={page.url.searchParams.get('template')}
/>
