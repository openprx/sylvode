import type { JSX } from "@solidjs/web";
import { onSettled, Show } from "solid-js";
import { LayoutRuntime, type RuntimeEnv } from "../../lib/layout/runtime";
import type { LayoutSpec } from "../../lib/layout/types";
import { useSite } from "../../lib/site/store";
import { RuntimeContext } from "./context";
import { LayoutNode } from "./layout-node";

export interface LayoutViewProps {
  spec: LayoutSpec;
  params: Record<string, unknown>;
  env: RuntimeEnv;
}

/** Renders one `sv.layout.v1` spec; the runtime is built once per mounted view. */
export function LayoutView(props: LayoutViewProps): JSX.Element {
  const site = useSite();
  const runtime = new LayoutRuntime(props.spec, props.params, props.env);
  onSettled(() => {
    void runtime.load();
  });
  return (
    <RuntimeContext value={runtime}>
      <div
        class="min-w-0"
        data-layout-key={props.spec.key}
        aria-busy={runtime.loading() ? "true" : undefined}
      >
        <LayoutNode node={props.spec.root} />
        <Show when={Object.keys(runtime.errors()).length > 0}>
          <p class="mt-2 text-meta text-secondary" data-layout-source-error>
            {site.t("layout.sourceError")}
          </p>
        </Show>
      </div>
    </RuntimeContext>
  );
}
