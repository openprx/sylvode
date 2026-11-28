<svelte:options runes />

<script lang="ts">
	import { get } from 'svelte/store';
	import { locale, t } from 'svelte-i18n';
	import Modal from '#lib/components/Modal.svelte';
	import { toast } from '#lib/stores/toast.js';
	import Button from '#lib/ui/button/Button.svelte';
	import Checkbox from '#lib/ui/checkbox/Checkbox.svelte';
	import SecurityWarning from '#lib/ui/security-warning/SecurityWarning.svelte';
	import StatusChip from '#lib/ui/status-chip/StatusChip.svelte';
	import { indexApi, type IndexPluginDetail, type IndexVersion } from './index-api';
	import {
		detailReason,
		fingerprint,
		formatSize,
		hostOf,
		IMPORT_ERROR_CODES,
		shortHash,
		type Role,
		type TrustPlan
	} from './index-logic';
	import { pickText } from './logic';

	/**
	 * The confirmation before a package is downloaded from a public index. It shows exactly what
	 * will be fetched and from where, and who signed it. A key this workspace has not pinned is
	 * pinned only by the workspace owner, who must tick that they trust it; an administrator sees
	 * why they cannot go on. A second key of an already trusted publisher is a key change and is
	 * shown with both fingerprints. The server repeats every check; this dialog only explains.
	 */
	let {
		open = $bindable(false),
		workspaceId,
		role,
		detail,
		version,
		plan,
		onclose,
		ondone,
		onrefresh
	}: {
		open?: boolean;
		workspaceId: string;
		role: Role;
		detail: IndexPluginDetail;
		version: IndexVersion;
		plan: TrustPlan;
		onclose: () => void;
		ondone: (digest: string) => void;
		/** The server disagreed with what the page knew about the key: reload the detail. */
		onrefresh: () => void;
	} = $props();

	let trust = $state(false);
	let busy = $state(false);
	let refusal = $state<string | null>(null);
	/** The server said the key is not pinned although the page thought it was. */
	let serverNeedsTrust = $state(false);
	let serverOwnerOnly = $state(false);

	const lang = $derived($locale ?? 'en');
	const title = $derived(pickText(detail.title, lang) || detail.id);
	const publisherName = $derived(pickText(detail.publisher.name, lang) || detail.publisher.id);
	const needsTrust = $derived(plan.needsTrust || serverNeedsTrust);
	const owner = $derived(role === 'owner');
	const ownerBlocked = $derived(
		(needsTrust && !owner && plan.blocked !== 'member') || serverOwnerOnly
	);
	const key = $derived(fingerprint(version.key_id));
	const reason = $derived.by(() => {
		if (plan.blocked === 'member') return $t('market.index.reasonMember');
		if (plan.blocked === 'key_revoked') return $t('market.index.reasonKeyRevoked');
		if (plan.blocked === 'unknown_key') return $t('market.index.reasonUnknownKey');
		if (ownerBlocked) return $t('market.index.reasonOwnerOnly');
		if (needsTrust && !trust) return $t('market.index.reasonTick');
		return undefined;
	});

	function refusalText(result: {
		message: string;
		error_code?: string;
		details?: unknown;
	}): string {
		const code = result.error_code;
		if (code && (IMPORT_ERROR_CODES as readonly string[]).includes(code)) {
			const sub = detailReason(result.details);
			const specific = sub ? `market.index.error.${code}.${sub}` : '';
			const translate = get(t);
			if (specific && translate(specific) !== specific) return translate(specific);
			const text = translate(`market.index.error.${code}.default`);
			if (text !== `market.index.error.${code}.default`) return text;
		}
		return result.message || get(t)('market.index.importFailed');
	}

	async function submit() {
		busy = true;
		refusal = null;
		const result = await indexApi.import(workspaceId, {
			source: detail.source,
			plugin_id: detail.id,
			version: version.version,
			...(needsTrust && owner && trust ? { trust_key_id: version.key_id } : {})
		});
		busy = false;
		if (result.code === 0 && result.data) {
			toast.success(get(t)('market.index.importedToast', { values: { name: title } }));
			ondone(result.data.digest);
			return;
		}
		if (result.error_code === 'publisher_untrusted') {
			serverNeedsTrust = true;
			onrefresh();
		}
		if (result.error_code === 'publisher_trust_owner_only') {
			serverOwnerOnly = true;
			onrefresh();
		}
		refusal = refusalText(result);
	}
</script>

<Modal
	bind:open
	title={$t('market.index.importTitle', { values: { name: title, version: version.version } })}
	maxWidthClass="max-w-2xl"
	{onclose}
>
	<div class="body" data-market-index-import-dialog>
		<dl class="facts">
			<div>
				<dt>{$t('market.index.source')}</dt>
				<dd>{detail.source}<span class="mono">{detail.source_url}</span></dd>
			</div>
			<div>
				<dt>{$t('market.index.downloadFrom')}</dt>
				<dd>
					<strong>{hostOf(version.url)}</strong>
					<span class="mono">{version.url}</span>
				</dd>
			</div>
			<div>
				<dt>{$t('market.index.size')}</dt>
				<dd>{formatSize(version.size, lang)}</dd>
			</div>
			<div>
				<dt>{$t('market.index.sha256')}</dt>
				<dd><span class="mono" title={version.sha256}>{shortHash(version.sha256)}</span></dd>
			</div>
			<div>
				<dt>{$t('market.index.publisher')}</dt>
				<dd>{publisherName} <span class="mono">{detail.publisher.id}</span></dd>
			</div>
			<div>
				<dt>{$t('market.index.signingKey')}</dt>
				<dd>
					<span class="mono" data-market-index-fingerprint>{key.algorithm} {key.groups}</span>
					{#if plan.pinned}
						<StatusChip tone="success">{$t('market.index.keyPinned')}</StatusChip>
					{:else}
						<StatusChip tone="warning">{$t('market.index.keyNotPinned')}</StatusChip>
					{/if}
				</dd>
			</div>
		</dl>

		{#if version.contents.custom_code}
			<p class="warn">{$t('market.customCode')}</p>
		{/if}

		{#if needsTrust}
			{#if plan.keyChange}
				<SecurityWarning tone="danger" title={$t('market.index.keyChangeTitle')}>
					<p>{$t('market.index.keyChangeBody', { values: { publisher: detail.publisher.id } })}</p>
					<p class="keys">
						{$t('market.index.keyOld')}
						{#each plan.pinnedKeyIds as old (old)}
							<span class="mono">{fingerprint(old).algorithm} {fingerprint(old).groups}</span>
						{/each}
					</p>
					<p class="keys">
						{$t('market.index.keyNew')}
						<span class="mono">{key.algorithm} {key.groups}</span>
					</p>
				</SecurityWarning>
			{:else}
				<SecurityWarning tone="warning" title={$t('market.index.untrustedTitle')}>
					<p>
						{$t('market.index.untrustedBody', { values: { publisher: detail.publisher.id } })}
					</p>
				</SecurityWarning>
			{/if}
			{#if owner}
				<Checkbox
					bind:checked={trust}
					label={$t('market.index.trustTick', { values: { publisher: detail.publisher.id } })}
				/>
			{:else}
				<p class="note" data-market-index-owner-only>{$t('market.index.ownerOnlyNote')}</p>
			{/if}
		{/if}

		{#if refusal}
			<p class="refusal" role="alert" data-market-index-refusal>{refusal}</p>
		{/if}

		<p class="note">{$t('market.index.afterImport')}</p>
	</div>

	{#snippet footer()}
		<Button variant="secondary" onclick={() => ((open = false), onclose())} disabled={busy}
			>{$t('market.index.cancel')}</Button
		>
		<Button
			variant="primary"
			loading={busy}
			loadingLabel={$t('market.index.importing')}
			disabled={reason !== undefined}
			disabledReason={reason}
			onclick={() => void submit()}
			data-market-index-confirm>{$t('market.index.importConfirm')}</Button
		>
	{/snippet}
</Modal>

<style>
	.body {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-6);
		font-family: var(--rf-font-sans-stack);
	}
	.facts {
		display: grid;
		gap: var(--rf-space-4);
		margin: 0;
		font-size: var(--rf-size-secondary);
	}
	.facts div {
		display: grid;
		grid-template-columns: 9rem minmax(0, 1fr);
		gap: var(--rf-space-4);
	}
	.facts dt {
		color: var(--rf-secondary);
	}
	.facts dd {
		margin: 0;
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
		align-items: center;
		overflow-wrap: anywhere;
	}
	.mono {
		font-family: var(--rf-font-mono-stack, ui-monospace, monospace);
		font-size: var(--rf-size-meta);
		color: var(--rf-secondary);
		overflow-wrap: anywhere;
	}
	.facts dd .mono[data-market-index-fingerprint],
	.keys .mono {
		color: var(--rf-text);
	}
	.keys {
		display: flex;
		flex-wrap: wrap;
		gap: var(--rf-space-3);
		align-items: center;
		margin: var(--rf-space-3) 0 0;
	}
	.warn {
		margin: 0;
		color: var(--rf-warning);
		font-size: var(--rf-size-secondary);
	}
	.note {
		margin: 0;
		font-size: var(--rf-size-secondary);
		color: var(--rf-secondary);
		max-width: 62ch;
	}
	.refusal {
		margin: 0;
		padding: var(--rf-space-4) var(--rf-space-5);
		border-radius: var(--rf-radius-card);
		background: var(--rf-danger-bg);
		color: var(--rf-danger);
		font-size: var(--rf-size-secondary);
	}
	@media (max-width: 560px) {
		.facts div {
			grid-template-columns: 1fr;
			gap: var(--rf-space-1);
		}
	}
</style>
