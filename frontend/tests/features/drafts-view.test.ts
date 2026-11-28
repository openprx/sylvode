/**
 * Helpers of the drafts review page (`lib/project/drafts/view.ts`).
 *
 * Run standalone: `bun test tests/features/drafts-view.test.ts`
 */
import { test } from 'node:test';
import nodeAssert from 'node:assert/strict';
import {
	canApprove,
	issueLine,
	riskTone,
	staleDigest,
	statusTone,
	targetLine
} from '../../src/lib/project/drafts/view';

test('a draft names its target in one line', () => {
	nodeAssert.equal(targetLine({ kind: 'form_schema', target: { form_id: 'f1' } }), 'form f1');
	nodeAssert.equal(
		targetLine({ kind: 'form_schema', target: { key: 'orders', name: 'Orders' } }),
		'new form orders'
	);
	nodeAssert.equal(
		targetLine({ kind: 'layout', target: { slot_id: 'form.record.detail', scope_kind: 'form' } }),
		'form.record.detail / form'
	);
});

test('a site page draft says whether it is a title, a new page or a page change', () => {
	const words = (key: string, values?: Record<string, string>): string =>
		`${key}${values?.route ? ` ${values.route}` : ''}`;
	const title = {
		kind: 'site_page',
		target: { site_id: 's', page_id: null, op: 'update' }
	} as const;
	nodeAssert.equal(targetLine(title, words), 'drafts.target.siteTitle');
	nodeAssert.equal(
		targetLine({ kind: 'site_page', target: { site_id: 's', page_id: null, op: 'create' } }, words),
		'drafts.target.pageCreate'
	);
	nodeAssert.equal(
		targetLine(
			{
				kind: 'site_page',
				target: { site_id: 's', page_id: 'p', op: 'delete', base_page: { route: '/help' } }
			},
			words
		),
		'drafts.target.page_delete /help'
	);
	nodeAssert.equal(
		targetLine(
			{
				kind: 'site_page',
				target: { site_id: 's', page_id: 'p', op: 'update', base_page: { route: '/a' } }
			},
			words
		),
		'drafts.target.page_update /a'
	);
	// Without words the kind is the line, as before.
	nodeAssert.equal(targetLine(title), 'site_page');
});

test('only an administrator approves a valid pending draft', () => {
	const pending = { status: 'pending', valid: true } as const;
	nodeAssert.equal(canApprove(pending, true), true);
	nodeAssert.equal(canApprove(pending, false), false);
	nodeAssert.equal(canApprove({ status: 'pending', valid: false }, true), false);
	nodeAssert.equal(canApprove({ status: 'approved', valid: true }, true), false);
});

test('tones follow the state and the risk', () => {
	nodeAssert.equal(statusTone('approved'), 'success');
	nodeAssert.equal(statusTone('pending'), 'warning');
	nodeAssert.equal(statusTone('rejected'), 'neutral');
	nodeAssert.equal(riskTone('read'), 'neutral');
	nodeAssert.equal(riskTone('write'), 'warning');
	nodeAssert.equal(riskTone('admin'), 'danger');
});

test('an issue is shown with its pointer and code', () => {
	nodeAssert.equal(
		issueLine({ path: '/fields/0', code: 'field_invalid' }),
		'/fields/0  field_invalid'
	);
	nodeAssert.equal(
		issueLine({ path: '', code: 'ref_invalid', message: 'form must be $bind.<name>' }),
		'/  ref_invalid: form must be $bind.<name>'
	);
});

test('a stale refusal carries the digest of the new state', () => {
	nodeAssert.equal(staleDigest({ review_digest: 'sha256:ab' }), 'sha256:ab');
	nodeAssert.equal(staleDigest({}), null);
	nodeAssert.equal(staleDigest(null), null);
});
