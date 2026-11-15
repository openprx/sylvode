<script lang="ts">
	import { t } from 'svelte-i18n';
	import { packageExportWarnings, PACKAGE_WARNING_KIND_KEYS } from '#lib/flow/package-warnings.js';
	let { warnings, scopeExport = false }: { warnings: unknown; scopeExport?: boolean } = $props();
	const entries = $derived(packageExportWarnings(warnings, scopeExport));
</script>

{#if entries.length > 0}
	<ul
		class="my-3 space-y-1 rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-950 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-100"
		data-testid="flow-package-export-warnings"
		role="status"
	>
		{#each entries as warning, index (index)}<li>
				{$t(warning.messageKey, {
					values: {
						kinds: warning.objectTypes.map((kind) => $t(PACKAGE_WARNING_KIND_KEYS[kind])).join(', ')
					}
				})}
			</li>{/each}
	</ul>
{/if}
