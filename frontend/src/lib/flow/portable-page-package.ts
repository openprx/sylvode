import type { ApiResult } from '#lib/api/client.js';

// The server's portable ZIP profile bounds the compressed and expanded archive.
export const MAX_PORTABLE_PAGE_BYTES = 32 * 1024 * 1024;

export function portablePageFailure(result: ApiResult<unknown>): string {
	const reason =
		typeof result.details === 'object' && result.details !== null && 'reason' in result.details
			? result.details.reason
			: null;
	switch (reason) {
		case 'portable_graph_packages_disabled':
		case 'portable_page_packages_disabled':
			return 'flow.portablePackage.unavailable';
		case 'portable_page_required':
			return 'flow.portablePackage.activeRequired';
		case 'portable_graph_target_not_selected':
			return 'flow.graphPackage.targetMissing';
		case 'portable_graph_forms_require_project':
			return 'flow.graphPackage.formsRequireProject';
		case 'portable_page_scope_mismatch':
			return 'flow.portablePackage.scopeMismatch';
		case 'portable_page_link_unavailable':
			return 'flow.portablePackage.linkUnavailable';
		case 'portable_page_unrepresented_reference':
			return 'flow.portablePackage.unsupportedReferences';
		case 'portable_page_asset_unavailable':
			return 'flow.portablePackage.assetUnavailable';
	}
	if (result.error_code === 'checksum_mismatch') return 'flow.portablePackage.checksumMismatch';
	if (result.error_code === 'limit_exceeded' || result.code === 413)
		return 'flow.portablePackage.tooLarge';
	if (result.code === 403 || result.code === 404) return 'flow.portablePackage.permission';
	if (result.error_code === 'unsupported_format' || result.code === 400 || result.code === 422)
		return 'flow.portablePackage.invalid';
	return 'flow.portablePackage.failed';
}
