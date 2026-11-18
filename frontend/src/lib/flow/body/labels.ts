/** Explicit bilingual keys: dynamic UI states must not manufacture unchecked i18n paths. */
export const BODY_UPLOAD_LABELS = {
	idle: 'flow.body.state.idle',
	uploading: 'flow.body.state.uploading',
	staged: 'flow.body.state.staged',
	inserting: 'flow.body.state.inserting',
	complete: 'flow.body.state.complete',
	failed: 'flow.body.state.failed',
	canceled: 'flow.body.state.canceled'
} as const;
export const BODY_ERROR_LABELS = {
	unavailable: 'flow.body.error.unavailable',
	size: 'flow.body.error.size',
	upload: 'flow.body.error.upload',
	network: 'flow.body.error.network',
	position: 'flow.body.error.position',
	insert: 'flow.body.error.insert',
	discard: 'flow.body.error.discard',
	prepare: 'flow.body.error.prepare',
	save: 'flow.body.error.save',
	cancel: 'flow.body.error.cancel',
	fileRequired: 'flow.body.error.fileRequired',
	titleRequired: 'flow.body.error.titleRequired',
	pageRequired: 'flow.body.error.pageRequired',
	pageUnavailable: 'flow.body.error.pageUnavailable',
	search: 'flow.body.error.search'
} as const;
export type BodyErrorReason = keyof typeof BODY_ERROR_LABELS;
export const BODY_MODAL_LABELS = {
	child: 'flow.body.newChild',
	link: 'flow.body.linkPage',
	details: 'flow.body.editDetails',
	image: 'flow.body.addImage',
	attachment: 'flow.body.addFile'
} as const;
export const BODY_INSERT_OPTIONS = [
	{
		action: 'image',
		labelKey: 'flow.body.addImage',
		searchAliasesKey: 'flow.body.searchAliases.image'
	},
	{
		action: 'attachment',
		labelKey: 'flow.body.addFile',
		searchAliasesKey: 'flow.body.searchAliases.attachment'
	},
	{
		action: 'child',
		labelKey: 'flow.body.newChild',
		searchAliasesKey: 'flow.body.searchAliases.child'
	},
	{
		action: 'link',
		labelKey: 'flow.body.linkPage',
		searchAliasesKey: 'flow.body.searchAliases.link'
	}
] as const;
