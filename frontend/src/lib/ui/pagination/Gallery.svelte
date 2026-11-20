<script lang="ts">
	import Pagination from './Pagination.svelte';

	let page = $state(2);
	let size = $state(20);
</script>

<section data-gallery-section="pagination">
	<h2>pagination</h2>
	<div class="col">
		<div data-gallery="pagination:default">
			<Pagination
				{page}
				pageSize={size}
				total={95}
				onpage={(next) => (page = next)}
				onpagesize={(next) => {
					size = next;
					page = 1;
				}}
			/>
		</div>
		<div data-gallery="pagination:first">
			<Pagination page={1} pageSize={20} total={95} onpage={() => {}} />
		</div>
		<div data-gallery="pagination:last">
			<Pagination page={5} pageSize={20} total={95} onpage={() => {}} />
		</div>
		<div data-gallery="pagination:loading">
			<Pagination page={2} pageSize={20} total={95} loading onpage={() => {}} />
		</div>
		<div data-gallery="pagination:unknown-total">
			<Pagination page={2} pageSize={50} total={null} hasMore onpage={() => {}} />
		</div>
		<div data-gallery="pagination:narrow" class="narrow">
			<Pagination page={3} pageSize={20} total={1240} onpage={() => {}} onpagesize={() => {}} />
		</div>
	</div>
</section>

<style>
	.col {
		display: flex;
		flex-direction: column;
		gap: var(--rf-space-5);
		max-width: 720px;
	}
	.narrow {
		max-width: 340px;
	}
</style>
