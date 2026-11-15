// Follow the accepted navigator order map on the current document, including recovery replacement.
interface NavigatorDocument {
	subscribe(listener: (event: { by: 'local' | 'import' | 'checkout' }) => void): () => void;
}
interface NavigatorBinding {
	readonly doc: NavigatorDocument;
	readonly docGeneration: { subscribe(listener: (generation: number) => void): () => void };
}
export function observeNavigatorDocument(
	binding: NavigatorBinding,
	changed: () => void,
	remote: () => void
): () => void {
	let unsubscribeDoc = () => {};
	let initialized = false;
	const unsubscribeGeneration = binding.docGeneration.subscribe(() => {
		unsubscribeDoc();
		unsubscribeDoc = binding.doc.subscribe((event) => {
			changed();
			if (event.by !== 'local') remote();
		});
		changed();
		if (initialized) remote();
		initialized = true;
	});
	return () => {
		unsubscribeGeneration();
		unsubscribeDoc();
	};
}
