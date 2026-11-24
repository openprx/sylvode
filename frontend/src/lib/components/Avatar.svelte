<script module lang="ts">
	/** What the avatar is called: the name, else nothing (shown as "?"). Never an id. */
	export function avatarName(name?: string): string {
		return name?.trim() || '';
	}
</script>

<script lang="ts">
	import { t } from 'svelte-i18n';
	import UiAvatar from '#lib/ui/avatar/Avatar.svelte';

	interface Props {
		name?: string;
		size?: 'sm' | 'md';
	}

	let { name, size = 'sm' }: Props = $props();

	const label = $derived(avatarName(name));
</script>

<UiAvatar name={label || '?'} {size} title={label || $t('people.someone')} />
