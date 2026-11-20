<script lang="ts">
	import { t } from 'svelte-i18n';
	import Avatar from './Avatar.svelte';
</script>

<section data-gallery-section="avatar">
	<h2>avatar</h2>
	<div class="row">
		<span data-gallery="avatar:person"><Avatar name={$t('ui.avatar.person')} /></span>
		<span data-gallery="avatar:agent"><Avatar name={$t('ui.avatar.agent')} kind="agent" /></span>
		<span data-gallery="avatar:group"><Avatar name={$t('ui.avatar.group')} kind="group" /></span>
		<span data-gallery="avatar:person-md"><Avatar name={$t('ui.avatar.person')} size="md" /></span>
		<span data-gallery="avatar:person-sm"><Avatar name={$t('ui.avatar.person')} size="sm" /></span>
		<span data-gallery="avatar:agent-sm"
			><Avatar name={$t('ui.avatar.agent')} kind="agent" size="sm" /></span
		>
	</div>
</section>

<style>
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-4);
		align-items: center;
	}
</style>
