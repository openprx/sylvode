<script lang="ts">
	import { page } from '$app/state';
	import { requireRouteParam } from '#lib/utils/route-params.js';
	import FlowSharedView from '#lib/components/flow/FlowSharedView.svelte';
	const shareId = requireRouteParam(page.params.shareId, 'shareId');
	const workspaceId = page.url.searchParams.get('workspace_id');
	const context = workspaceId
		? { workspace_id: workspaceId, project_id: page.url.searchParams.get('project_id') }
		: undefined;
</script>

<FlowSharedView {shareId} {context} />
