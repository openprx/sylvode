<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import DraftsSection from '#lib/project/drafts/DraftsSection.svelte';
	import InfoButton from '#lib/ui/info-button/InfoButton.svelte';

	/**
	 * What agents wrote for this site while it is published. The review is the project's drafts
	 * review (approve with the digest the reviewer saw, reject with a reason); this tab only
	 * narrows it to the site and says in plain words why a change waits here.
	 */
	let { projectId, siteId }: { projectId: string; siteId: string } = $props();

	const RULES = [
		'pageChange',
		'layout_on_published_site',
		'form_on_published_site',
		'public_site_text_requires_human',
		'draft_stale'
	] as const;
</script>

<div class="drafts-tab" data-testid="site-drafts">
	<div class="lead">
		<p>{$t('sites.drafts.lead')}</p>
		<InfoButton label={$t('sites.drafts.whyLabel')}>
			<div class="rules" data-testid="draft-rules">
				<strong>{$t('sites.drafts.whyTitle')}</strong>
				<ul>
					{#each RULES as rule (rule)}
						<li>{$t(`sites.drafts.rule.${rule}`)}</li>
					{/each}
				</ul>
			</div>
		</InfoButton>
	</div>
	<DraftsSection {projectId} {siteId} />
</div>

<style>
	.drafts-tab {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
	}
	.lead {
		display: flex;
		align-items: center;
		gap: var(--rf-space-3);
		color: var(--rf-secondary);
		font-size: var(--rf-size-secondary);
	}
	.lead p {
		margin: 0;
	}
	.rules ul {
		margin: var(--rf-space-3) 0 0;
		padding-inline-start: var(--rf-space-7);
	}
</style>
