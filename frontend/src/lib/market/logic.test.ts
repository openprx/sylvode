import { Suite, assertDeepEqual, assertEqual, finish } from '../../../tests/support/harness';
import type { PreviewRequest } from './api';
import { dataAccess, pickText, riskTone } from './logic';

function request(capability: string, risk: PreviewRequest['risk'], form?: string): PreviewRequest {
	return {
		capability,
		selector: { form: null },
		form_bind: form ?? null,
		risk,
		label_key: '',
		new: false
	};
}

const suite = new Suite('market-logic');

suite.check('groups requests per bound form into reads and writes', () => {
	const rows = dataAccess(
		[
			request('forms.records.read', 'read', 'ticket'),
			request('forms.records.create', 'write', 'ticket'),
			request('forms.forms.read', 'read')
		],
		(capability) => capability
	);
	assertDeepEqual(
		rows,
		[
			{ target: 'ticket', reads: ['forms.records.read'], writes: ['forms.records.create'] },
			{ target: null, reads: ['forms.forms.read'], writes: [] }
		],
		'per form'
	);
});

suite.check('picks the text in the viewer language, then English', () => {
	assertEqual(pickText({ en: 'Desk', zh: '服务台' }, 'zh-CN'), '服务台', 'base language');
	assertEqual(pickText({ en: 'Desk' }, 'zh'), 'Desk', 'english fallback');
	assertEqual(pickText(null, 'en'), '', 'none');
});

suite.check('marks reading quiet and everything else loud', () => {
	assertEqual(riskTone('read'), 'success', 'read');
	assertEqual(riskTone('write'), 'warning', 'write');
	assertEqual(riskTone('secret'), 'danger', 'secret');
});

finish(suite.result());
