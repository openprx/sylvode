const protocol = 1;

if (window.parent !== window) {
	window.addEventListener('message', initialize);
	window.parent.postMessage({ protocol, type: 'sylvode-plugin-shell-ready' }, '*');
}

function initialize(event) {
	const { data, source, ports } = event;
	if (
		source !== window.parent || data?.protocol !== protocol ||
		data.type !== 'sylvode-plugin-shell-init' || typeof data.html !== 'string' || ports.length !== 1 ||
		data.bridge !== 2 || data.context === null || typeof data.context !== 'object'
	) return;
	window.removeEventListener('message', initialize);

	const frame = document.createElement('iframe');
	frame.title = 'Plugin content';
	frame.setAttribute('sandbox', 'allow-scripts');
	frame.referrerPolicy = 'no-referrer';
	const policy = `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'">`;
	frame.srcdoc = '<!doctype html>' + policy + data.html.replace(/^\s*<!doctype html>/i, '');
	let connected = false;
	frame.addEventListener('load', () => {
		if (connected) {
			frame.remove();
			window.parent.postMessage({ protocol, type: 'sylvode-plugin-shell-invalidated' }, '*');
			return;
		}
		connected = true;
		frame.contentWindow.postMessage({
			protocol: 2,
			type: 'sylvode-plugin-connect',
			grants: data.grants,
			context: data.context
		}, '*', ports);
	});
	document.body.append(frame);
}
