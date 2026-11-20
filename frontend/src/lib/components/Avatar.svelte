<script module lang="ts">
	/** What the avatar is called: the name, else the user id, else nothing (shown as "?"). */
	export function avatarName(name?: string, userId?: string): string {
		return name?.trim() || userId?.trim() || '';
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import UiAvatar from '#lib/ui/avatar/Avatar.svelte';

	interface Props {
		name?: string;
		userId?: string;
		size?: 'sm' | 'md';
	}

	let { name, userId, size = 'sm' }: Props = $props();

	const label = $derived(avatarName(name, userId));
</script>

<UiAvatar name={label || '?'} {size} title={label || $t('common.unassigned')} />
