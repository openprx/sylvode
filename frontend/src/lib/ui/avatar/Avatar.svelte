<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import Users from '@lucide/svelte/icons/users';
	import { initialsOf, shapeOf, type AvatarKind, type AvatarSize } from './avatar-state';

	interface Props extends Omit<HTMLAttributes<HTMLSpanElement>, 'children' | 'class'> {
		/** The person, agent or group name: source of the initials and the accessible name. */
		name: string;
		/** A person is a circle, an agent a rounded square, a group a circle with a users glyph. */
		kind?: AvatarKind;
		/** 40 by default; 32 and 24 for dense rows and avatar clusters. */
		size?: AvatarSize;
		/** Use when the name is already printed next to the avatar. */
		decorative?: boolean;
	}

	let { name, kind = 'person', size = 'lg', decorative = false, ...rest }: Props = $props();

	const initials = $derived(initialsOf(name));
	const shape = $derived(shapeOf(kind));
	const glyph = $derived(size === 'lg' ? 18 : size === 'md' ? 16 : 14);
</script>

<span
	{...rest}
	class="ui-avatar kind-{kind} size-{size} shape-{shape}"
	role={decorative ? undefined : 'img'}
	aria-label={decorative ? undefined : name}
	aria-hidden={decorative ? 'true' : undefined}
	data-ui="avatar"
	data-kind={kind}
>
	{#if kind === 'group'}
		<Users size={glyph} aria-hidden="true" />
	{:else}
		{initials}
	{/if}
</span>

<style>
	.ui-avatar {
		display: inline-grid;
		place-items: center;
		flex: none;
		overflow: hidden;
		background: var(--rf-avatar-person);
		color: var(--rf-on-avatar-person);
		font-family: var(--rf-font-sans-stack);
		font-weight: var(--rf-weight-semibold);
		line-height: 1;
		user-select: none;
	}
	.ui-avatar.shape-circle {
		border-radius: var(--rf-radius-full);
	}
	/* The rounded square keeps a radius of a quarter of its side: 10 at 40, 6 at 24. */
	.ui-avatar.shape-rounded-square {
		border-radius: 25%;
		background: var(--rf-agent-av);
		color: var(--rf-on-agent-av);
	}
	.size-lg {
		width: 40px;
		height: 40px;
		/* Initial inside a 40px avatar (specification 1.2, literal 16 / 600). */
		font-size: 16px;
	}
	.size-md {
		width: 32px;
		height: 32px;
		font-size: var(--rf-size-secondary);
	}
	.size-sm {
		width: 24px;
		height: 24px;
		font-size: var(--rf-size-caption);
	}
</style>
