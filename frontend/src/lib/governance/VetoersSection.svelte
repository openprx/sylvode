<svelte:options runes />

<script lang="ts">
	import { t } from 'svelte-i18n';
	import { describeResult } from '#lib/i18n/describe.js';
	import { membersApi, type WorkspaceMember } from '#lib/api/members.js';
	import { vetoApi, type VetoerGrant, type VetoerScope } from '#lib/api/veto.js';
	import { toast } from '#lib/stores/toast.js';
	import { confirmAction } from '#lib/ui/dialog/confirm.js';
	import { normalizeDomainKey, vetoerFormProblem } from './veto-rules.js';

	interface Props {
		projectId: string;
		workspaceId: string;
	}

	let { projectId, workspaceId }: Props = $props();

	let vetoers = $state<VetoerScope[]>([]);
	let members = $state<WorkspaceMember[]>([]);
	let loading = $state(true);
	let loadError = $state('');
	let adding = $state(false);
	let userId = $state('');
	let domain = $state('');
	let grant = $state<VetoerGrant>('manual_grant');
	let formError = $state('');
	let removingKey = $state('');

	const GRANTS: VetoerGrant[] = ['manual_grant', 'trust_score'];
	const memberById = $derived(new Map(members.map((member) => [member.user_id, member])));
	const rowKey = (row: Pick<VetoerScope, 'user_id' | 'domain'>) => `${row.user_id}|${row.domain}`;

	function nameOf(id: string): string {
		return memberById.get(id)?.name || $t('vetoers.unknownUser');
	}

	// The list and the member choices reload when the selected project changes; a late answer for
	// a project that is no longer selected is dropped.
	$effect(() => {
		const project = projectId;
		const workspace = workspaceId;
		let current = true;
		loading = true;
		loadError = '';
		userId = '';
		domain = '';
		formError = '';
		void (async () => {
			const [listed, memberList] = await Promise.all([
				vetoApi.listVetoers({ project_id: project }),
				membersApi.list(workspace)
			]);
			if (!current) return;
			if (listed.code !== 0 || !listed.data) {
				vetoers = [];
				loadError = describeResult(listed, $t('vetoers.loadFailed'));
			} else {
				vetoers = listed.data.items ?? [];
			}
			if (memberList.code === 0 && memberList.data) members = memberList.data.items ?? [];
			else {
				members = [];
				if (!loadError) loadError = describeResult(memberList, $t('vetoers.membersFailed'));
			}
			loading = false;
		})();
		return () => {
			current = false;
		};
	});

	async function reload() {
		const listed = await vetoApi.listVetoers({ project_id: projectId });
		if (listed.code === 0 && listed.data) vetoers = listed.data.items ?? [];
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		if (adding) return;
		const problem = vetoerFormProblem({ userId, domain }, vetoers);
		formError = problem ? $t(`vetoers.problem.${problem}`) : '';
		if (problem) return;
		adding = true;
		const response = await vetoApi.createVetoer({
			user_id: userId,
			project_id: projectId,
			domain: normalizeDomainKey(domain),
			granted_by: grant
		});
		adding = false;
		if (response.code !== 0) {
			formError = describeResult(response, $t('vetoers.addFailed'));
			return;
		}
		toast.success($t('vetoers.added'));
		userId = '';
		domain = '';
		await reload();
	}

	async function remove(row: VetoerScope) {
		const name = nameOf(row.user_id);
		const confirmed = await confirmAction({
			tone: 'destructive',
			title: $t('vetoers.removeTitle'),
			body: $t('vetoers.removeBody', { values: { name, domain: row.domain } }),
			verb: 'remove',
			object: $t('vetoers.removeObject', { values: { name, domain: row.domain } })
		});
		if (!confirmed) return;
		removingKey = rowKey(row);
		const response = await vetoApi.deleteVetoer({
			user_id: row.user_id,
			project_id: row.project_id,
			domain: row.domain
		});
		removingKey = '';
		if (response.code !== 0) {
			toast.error(describeResult(response, $t('vetoers.removeFailed')));
			return;
		}
		toast.success($t('vetoers.removed'));
		await reload();
	}

	function grantedOn(value: string): string {
		const date = new Date(value);
		return Number.isNaN(date.getTime()) ? '' : date.toLocaleDateString();
	}
</script>

<section class="rounded-card border border-line bg-raised p-5" aria-labelledby="vetoers-heading">
	<h2 id="vetoers-heading" class="text-lg font-semibold text-fg">{$t('vetoers.title')}</h2>
	<p class="mt-1 text-sm text-fg-muted">{$t('vetoers.subtitle')}</p>

	{#if loading}
		<p class="mt-4 text-sm text-fg-muted">{$t('common.loading')}</p>
	{:else}
		{#if loadError}
			<p role="alert" class="mt-4 text-sm text-danger">{loadError}</p>
		{/if}

		{#if vetoers.length === 0}
			<p class="mt-4 text-sm text-fg-muted">{$t('vetoers.empty')}</p>
		{:else}
			<ul class="mt-4 divide-y divide-line" data-testid="vetoer-list">
				{#each vetoers as row (rowKey(row))}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2">
						<div class="min-w-0">
							<p class="truncate text-sm font-medium text-fg">
								{nameOf(row.user_id)}
								{#if memberById.get(row.user_id)?.entity_type === 'bot'}
									<span class="ml-1 rounded bg-surface px-1.5 py-0.5 text-xs text-fg-muted"
										>{$t('vetoers.bot')}</span
									>
								{/if}
							</p>
							<p class="text-xs text-fg-muted">
								{$t('vetoers.domain')}: {row.domain} · {$t(`vetoers.grantedBy.${row.granted_by}`)}
								· {grantedOn(row.granted_at)}
							</p>
						</div>
						<button
							type="button"
							onclick={() => remove(row)}
							disabled={removingKey === rowKey(row)}
							class="rounded-control border border-fg px-3 py-1.5 text-xs text-fg hover:bg-surface disabled:cursor-not-allowed disabled:text-fg-muted"
						>
							{$t('vetoers.remove')}
						</button>
					</li>
				{/each}
			</ul>
		{/if}

		<form class="mt-5 grid grid-cols-1 gap-3 md:grid-cols-3" onsubmit={add}>
			<div>
				<label for="vetoer-member" class="mb-1 block text-sm text-fg-muted"
					>{$t('vetoers.member')}</label
				>
				<select
					id="vetoer-member"
					bind:value={userId}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					<option value="">{$t('vetoers.chooseMember')}</option>
					{#each members as member (member.user_id)}
						<option value={member.user_id}>{member.name}</option>
					{/each}
				</select>
			</div>
			<div>
				<label for="vetoer-domain" class="mb-1 block text-sm text-fg-muted"
					>{$t('vetoers.domain')}</label
				>
				<input
					id="vetoer-domain"
					type="text"
					bind:value={domain}
					placeholder={$t('vetoers.domainPlaceholder')}
					aria-describedby="vetoer-domain-hint"
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg placeholder:text-fg-muted"
				/>
				<p id="vetoer-domain-hint" class="mt-1 text-xs text-fg-muted">{$t('vetoers.domainHint')}</p>
			</div>
			<div>
				<label for="vetoer-grant" class="mb-1 block text-sm text-fg-muted"
					>{$t('vetoers.grant')}</label
				>
				<select
					id="vetoer-grant"
					bind:value={grant}
					class="w-full rounded-control border border-input bg-field px-3 py-2 text-sm text-fg"
				>
					{#each GRANTS as option (option)}
						<option value={option}>{$t(`vetoers.grantedBy.${option}`)}</option>
					{/each}
				</select>
			</div>
			{#if formError}
				<p role="alert" class="text-sm text-danger md:col-span-3">{formError}</p>
			{/if}
			<div class="flex justify-end md:col-span-3">
				<button
					type="submit"
					disabled={adding}
					class="rounded-control rf-filled bg-primary px-4 py-2 text-sm text-on-primary disabled:bg-surface disabled:text-fg-muted disabled:cursor-not-allowed"
				>
					{adding ? $t('vetoers.adding') : $t('vetoers.add')}
				</button>
			</div>
		</form>
	{/if}
</section>
