<script lang="ts">
	import { t } from 'svelte-i18n';
	import Radio from './Radio.svelte';
	import RadioGroup from './RadioGroup.svelte';

	let value = $state('b');
</script>

<section data-gallery-section="radio">
	<h2>radio</h2>
	<div data-gallery="radio:group">
		<RadioGroup label={$t('ui.gallery.sample')} bind:value>
			<Radio value="a" label="a" />
			<Radio value="b" label="b" />
			<Radio value="c" label="c" disabled />
			<Radio value="d" label="d" />
		</RadioGroup>
	</div>
</section>
