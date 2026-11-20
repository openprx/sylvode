<script module lang="ts">
	import type { ButtonVariant } from '#lib/ui/button/button-state.js';

	export type LegacyButtonVariant = 'primary' | 'secondary' | 'danger' | 'dangerSolid' | 'ghost';

	/**
	 * Old variant names onto the new primitive. `secondary` keeps its role (the quiet default next
	 * to a primary action), `ghost` is the borderless text button, `danger` is the quiet danger
	 * button and `dangerSolid` the filled one.
	 */
	export const BUTTON_VARIANTS: Record<LegacyButtonVariant, ButtonVariant> = {
		primary: 'primary',
		secondary: 'secondary',
		danger: 'danger-quiet',
		dangerSolid: 'danger',
		ghost: 'text'
	};

	export function mapButtonVariant(variant: string | undefined): ButtonVariant {
		return BUTTON_VARIANTS[(variant ?? 'primary') as LegacyButtonVariant] ?? 'secondary';
	}
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLButtonAttributes } from 'svelte/elements';
	import UiButton from '#lib/ui/button/Button.svelte';

	interface Props extends Omit<HTMLButtonAttributes, 'children' | 'class' | 'onclick'> {
		variant?: LegacyButtonVariant;
		size?: 'sm' | 'md' | 'lg';
		disabled?: boolean;
		loading?: boolean;
		type?: 'button' | 'submit' | 'reset';
		onclick?: HTMLButtonAttributes['onclick'];
		'aria-expanded'?: boolean;
		/** Shown next to a disabled button when the page can say why. */
		disabledReason?: string;
		children?: Snippet;
	}

	let {
		variant = 'primary',
		size = 'md',
		disabled = false,
		loading = false,
		type = 'button',
		onclick,
		children,
		...rest
	}: Props = $props();
</script>

{#snippet nothing()}{/snippet}

<UiButton
	{...rest}
	variant={mapButtonVariant(variant)}
	{size}
	{disabled}
	{loading}
	{type}
	{onclick}
	children={children ?? nothing}
/>
