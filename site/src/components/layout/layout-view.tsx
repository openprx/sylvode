import type { JSX } from "@solidjs/web";
import { onSettled, Show } from "solid-js";
import { resolveText } from "../../lib/layout/refs";
import { LayoutRuntime, type RuntimeEnv } from "../../lib/layout/runtime";
import type { LayoutSpec } from "../../lib/layout/types";
import { useSite } from "../../lib/site/store";
import { ListFailed } from "../list-state";
import { PageTitle } from "../page-title";
import { RuntimeContext } from "./context";
import { LayoutNode } from "./layout-node";

export interface LayoutViewProps {
  spec: LayoutSpec;
  params: Record<string, unknown>;
  env: RuntimeEnv;
}

/** Renders one `sv.layout.v1` spec; the runtime is built once per mounted view. */
export function LayoutView(props: LayoutViewProps): JSX.Element {
  const runtime = new LayoutRuntime(props.spec, props.params, props.env);
  onSettled(() => {
    void runtime.load();
  });
  const site = useSite();
  // Tables, stats and field displays show their own failure; a failed form source has no node of
  // its own to do so, so the view reports it.
  const formSourceFailed = () =>
    Object.keys(runtime.errors()).some((name) => props.spec.data?.[name]?.source === "form");
  const heading = () =>
    props.spec.title ? resolveText(props.spec.title, runtime.scope(), runtime.locale) : "";
  // The layout title is the page heading unless the root node sets `hide_title: true`.
  const showHeading = () => props.spec.root.props?.hide_title !== true;
  return (
    <RuntimeContext value={runtime}>
      <div
        class="min-w-0"
        data-layout-key={props.spec.key}
        aria-busy={runtime.loading() ? "true" : undefined}
      >
        <PageTitle title={heading()} />
        <Show when={heading() && showHeading()}>
          <h1 class="m-0 mb-4 text-page font-semibold text-text">{heading()}</h1>
        </Show>
        <LayoutNode node={props.spec.root} />
        <Show when={formSourceFailed()}>
          <div class="mt-2" data-layout-source-error>
            <ListFailed
              message={site.t("layout.sourceError")}
              onRetry={() => void runtime.retry()}
            />
          </div>
        </Show>
      </div>
    </RuntimeContext>
  );
}
