<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import { t } from 'svelte-i18n';
	import {
		bridgeProtocol,
		bridgeProtocolV2,
		clampFrameHeight,
		isBridgeSignal,
		parseBridgeV2,
		pluginRequest,
		shellInitMessage,
		verifyPanelHtml,
		type BridgeErrorCode,
		type PluginHostContext as Context
	} from '#lib/plugins/bridge.js';
	import {
		createFrameLifecycle,
		suspendGraceMs,
		type FrameLifecycle
	} from '#lib/plugins/frame-lifecycle.js';
	import { createHostDispatcher, type HostOutcome } from '#lib/plugins/host-methods.js';

	let {
		html,
		webSha256,
		bridge,
		grants,
		context,
		placement,
		transport,
		title,
		minHeight,
		maxHeight,
		onClose,
		onRetired,
		onConfirmApply
	}: {
		html: string;
		webSha256: string;
		bridge: 1 | 2;
		grants: string[];
		context?: Context;
		placement?: { id: string; slot: string };
		transport: (
			method: string,
			params: Record<string, unknown>,
			requestId: string
		) => Promise<{ ok: true; result: unknown } | { ok: false; code: string }>;
		title: string;
		minHeight: number;
		maxHeight: number;
		onClose: (reason: '' | string) => void;
		/**
		 * Called when this host runs again after it retired its frame silently (see
		 * `retiredSilently`). Read once, when the host opens. Without it, `onClose` gets the
		 * suspended reason, so the caller never shows an empty host.
		 */
		onRetired?: () => void;
		onConfirmApply?: () => Promise<boolean>;
	} = $props();

	// Everything that defines what runs in the frame is fixed when the host opens. A later
	// change to any of it (new version, new grants, new digest) suspends and closes the frame;
	// callers mount a fresh host for the new state.
	const identity = (values: { html: string; webSha256: string; bridge: 1 | 2; grants: string[] }) =>
		JSON.stringify([values.html, values.webSha256, values.bridge, [...values.grants].sort()]);
	const opened = untrack(() => ({
		html,
		webSha256,
		bridge,
		grants: [...grants],
		placement: placement ? { id: placement.id, slot: placement.slot } : undefined
	}));
	const openedIdentity = identity(opened);
	const legacy = opened.bridge === 1;
	const reasons = legacy
		? {
				integrity: 'plugins.integrityFailed',
				load: 'plugins.loadFailed',
				suspended: 'plugins.accessChanged',
				changed: 'plugins.versionChanged',
				timeout: 'plugins.actionTimedOut'
			}
		: {
				integrity: 'plugins.host.integrityFailed',
				load: 'plugins.host.loadFailed',
				suspended: 'plugins.host.suspended',
				changed: 'plugins.host.suspended',
				timeout: 'plugins.actionTimedOut'
			};
	const messages: Record<BridgeErrorCode, string> = {
		method_unavailable: legacy ? 'plugins.actionUnavailable' : 'plugins.host.methodUnavailable',
		invalid_params: 'plugins.host.invalidParams',
		busy: 'plugins.actionBusy',
		denied: 'plugins.accessChanged',
		stale: 'plugins.versionChanged',
		not_found: 'plugins.host.notFound',
		rate_limited: 'plugins.host.rateLimited',
		timeout: 'plugins.actionTimedOut',
		canceled: 'plugins.actionCanceled',
		preview_required: 'plugins.previewRequired',
		failed: 'plugins.actionFailed'
	};

	let stage = $state<'verifying' | 'opening' | 'ready' | 'closed'>('verifying');
	let height = $state(untrack(() => clampFrameHeight(minHeight, minHeight, maxHeight)));
	let frame: HTMLIFrameElement | undefined = $state();
	let port: MessagePort | undefined;
	let lifecycle: FrameLifecycle | undefined;
	let loadTimer: ReturnType<typeof setTimeout> | undefined;
	let finished = false;
	let sentContext = '';
	/**
	 * Set when the frame was retired because the caller unmounted or re-keyed this host (no
	 * `onClose`). Svelte keeps such a host alive during the outro grace period and resumes it,
	 * instead of mounting a new one, when the same block becomes current again before the outro
	 * ends (for example browser Back within 300 ms). A resumed host has no frame and can never get
	 * one, so it tells its caller as soon as it runs again; a host that is destroyed never does.
	 */
	let retiredSilently = $state(false);
	const retired = untrack(() => onRetired);

	const dispatcher = createHostDispatcher({
		bridge: opened.bridge,
		grants: () => opened.grants,
		context: () => currentContext(),
		transport: (method, params, requestId) => transport(method, params, requestId),
		confirmApply: () => (onConfirmApply ? onConfirmApply() : Promise.resolve(false))
	});

	function currentContext(): Context | undefined {
		return context ? ($state.snapshot(context) as Context) : undefined;
	}

	/**
	 * Stops serving the frame and retires its port. With a reason on a protocol 2 frame whose port
	 * is open, the plugin first gets `suspended` and a short grace period (see frame-lifecycle.ts):
	 * serving stops immediately, only the removal is delayed. `notify` is false when the caller
	 * unmounted us.
	 */
	function finish(reason: string, notify = true) {
		if (finished) return;
		finished = true;
		dispatcher.dispose();
		clearTimeout(loadTimer);
		if (!legacy && reason !== '' && stage !== 'verifying')
			lifecycle?.suspend({
				protocol: bridgeProtocolV2,
				type: 'event',
				name: 'suspended',
				data: null
			});
		else lifecycle?.close();
		port = undefined;
		stage = 'closed';
		if (notify) onClose(reason);
		else retiredSilently = true;
	}

	/**
	 * Outro of the frame element. It runs when the frame is removed, including when a parent
	 * unmounts or re-keys this component without going through finish(): Svelte then keeps the
	 * element mounted for the returned duration, and the lifecycle closes it at the latest then.
	 */
	function holdWhileSuspending(node: HTMLElement) {
		finish(reasons.suspended, false);
		if (lifecycle?.state !== 'suspending') return { duration: 0 };
		node.inert = true;
		node.style.display = 'none';
		return { duration: suspendGraceMs };
	}

	onMount(() => {
		let active = true;
		void (async () => {
			if (!legacy && !currentContext()) {
				finish(reasons.load);
				return;
			}
			let verified = false;
			try {
				verified = await verifyPanelHtml({ html: opened.html, web_sha256: opened.webSha256 });
			} catch {
				/* Unavailable integrity verification fails closed. */
			}
			if (!active || finished) return;
			if (!verified) {
				finish(reasons.integrity);
				return;
			}
			stage = 'opening';
			loadTimer = setTimeout(() => finish(reasons.load), 7000);
		})();
		return () => {
			active = false;
			finish(reasons.suspended, false);
			lifecycle?.close();
		};
	});

	// Effects of a host that is being removed (paused for its outro, or destroyed) do not run, so
	// this only fires once the host is live again after a silent retire.
	$effect(() => {
		if (!retiredSilently) return;
		untrack(() => {
			retiredSilently = false;
			if (retired) retired();
			else onClose(reasons.suspended);
		});
	});

	$effect(() => {
		const next = identity({ html, webSha256, bridge, grants });
		if (next !== openedIdentity) untrack(() => finish(reasons.changed));
	});

	$effect(() => {
		if (legacy || !context) return;
		const snapshot = JSON.stringify($state.snapshot(context));
		untrack(() => {
			if (stage !== 'ready' || !port || snapshot === sentContext) return;
			sentContext = snapshot;
			port.postMessage({
				protocol: bridgeProtocolV2,
				type: 'event',
				name: 'context',
				data: JSON.parse(snapshot)
			});
		});
	});

	function onWindowMessage(event: MessageEvent) {
		if (finished || !frame || event.source !== frame.contentWindow || event.origin !== 'null')
			return;
		if (isBridgeSignal(event.data, 'sylvode-plugin-shell-invalidated')) {
			finish(reasons.load);
			return;
		}
		if (stage !== 'opening' || port || !isBridgeSignal(event.data, 'sylvode-plugin-shell-ready'))
			return;
		const initContext = currentContext();
		if (!legacy && !initContext) {
			finish(reasons.load);
			return;
		}
		const channel = new MessageChannel();
		port = channel.port1;
		port.onmessage = ({ data }) => (legacy ? receiveLegacy(data) : receive(data));
		const node = frame;
		lifecycle = createFrameLifecycle({
			port,
			dispose: () => dispatcher.dispose(),
			removeFrame: () => node.remove()
		});
		if (initContext) sentContext = JSON.stringify(initContext);
		const init =
			legacy || !initContext
				? shellInitMessage({
						bridge: 1,
						html: opened.html,
						grants: [...opened.grants],
						...(opened.placement ? { placement: { ...opened.placement } } : {})
					})
				: shellInitMessage({
						bridge: 2,
						html: opened.html,
						grants: [...opened.grants],
						context: initContext
					});
		frame.contentWindow?.postMessage(init, '*', [channel.port2]);
	}

	function markReady() {
		clearTimeout(loadTimer);
		stage = 'ready';
		// A context change that arrived during the handshake is delivered once the plugin is ready.
		const latest = currentContext();
		const snapshot = latest ? JSON.stringify(latest) : '';
		if (legacy || !port || !latest || snapshot === sentContext) return;
		sentContext = snapshot;
		port.postMessage({ protocol: bridgeProtocolV2, type: 'event', name: 'context', data: latest });
	}

	function receiveLegacy(data: unknown) {
		if (finished) return;
		if (stage === 'opening' && isBridgeSignal(data, 'ready')) {
			markReady();
			return;
		}
		if (stage !== 'ready') return;
		if (isBridgeSignal(data, 'close')) {
			finish('');
			return;
		}
		const request = pluginRequest(data);
		if (!request) return;
		void dispatcher
			.handle(request.requestId, request.method, {})
			.then((outcome) => answer(request.requestId, outcome));
	}

	function receive(data: unknown) {
		if (finished) return;
		const message = parseBridgeV2(data);
		if (!message) return;
		if (message.type === 'ready') {
			if (stage === 'opening') markReady();
			return;
		}
		if (stage !== 'ready') return;
		if (message.type === 'close') finish('');
		else if (message.type === 'resize')
			height = clampFrameHeight(message.height, minHeight, maxHeight);
		else
			void dispatcher
				.handle(message.requestId, message.method, message.params)
				.then((outcome) => answer(message.requestId, outcome));
	}

	function answer(requestId: string, outcome: HostOutcome) {
		if (finished || !port) return;
		if (!outcome.ok && outcome.close === 'timeout' && legacy) {
			finish(reasons.timeout);
			return;
		}
		if (legacy)
			port.postMessage({
				protocol: bridgeProtocol,
				type: 'result',
				request_id: requestId,
				...(outcome.ok
					? { ok: true, result: outcome.result }
					: { ok: false, message: $t(messages[outcome.code]) })
			});
		else
			port.postMessage({
				protocol: bridgeProtocolV2,
				type: 'result',
				request_id: requestId,
				...(outcome.ok
					? { ok: true, result: outcome.result }
					: { ok: false, error: { code: outcome.code, message: $t(messages[outcome.code]) } })
			});
		if (!outcome.ok && outcome.close === 'suspend') finish(reasons.suspended);
		else if (!outcome.ok && outcome.close === 'timeout') finish(reasons.timeout);
	}
</script>

<svelte:window onmessage={onWindowMessage} />

{#if stage === 'verifying' || stage === 'opening'}
	<p class="px-3 pb-2 text-sm text-slate-600 dark:text-slate-300" role="status">
		{$t(legacy ? 'plugins.loadingPanel' : 'plugins.host.loading')}
	</p>
{/if}
{#if stage === 'opening' || stage === 'ready'}
	<iframe
		bind:this={frame}
		{title}
		src="/plugin-host/frame.html"
		out:holdWhileSuspending|global
		sandbox="allow-scripts"
		referrerpolicy="no-referrer"
		class="block w-full border-0"
		style:height={`${height}px`}
		hidden={stage !== 'ready'}
	></iframe>
{/if}
