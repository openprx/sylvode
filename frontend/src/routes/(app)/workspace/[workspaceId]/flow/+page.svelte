<script lang="ts">
	import { t } from 'svelte-i18n';
</script>

<div class="flex flex-1 items-center justify-center p-8 text-center">
	<div>
		<p class="mb-1 text-sm font-medium text-fg">
			{$t('flow.nav.empty')}
		</p>
		<p class="text-sm text-fg-muted">{$t('flow.nav.createFirst')}</p>
	</div>
</div>
