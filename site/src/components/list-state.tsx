import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import { useSite } from "../lib/site/store";
import { Button } from "./ui";

export type ListStatus = "loading" | "ready" | "failed";

/** Static placeholder in the shape of the final table; announced once, never read bar by bar. */
export function ListSkeleton(props: { columns?: number; rows?: number }): JSX.Element {
  const site = useSite();
  const columns = () => Array.from({ length: Math.min(Math.max(props.columns ?? 3, 1), 8) });
  const rows = () => Array.from({ length: props.rows ?? 4 });
  return (
    <div class="flex flex-col gap-3" role="status" aria-busy="true" data-list-state="loading">
      <span class="sr-only">{site.t("app.loading")}</span>
      <div aria-hidden="true" class="flex flex-col gap-3">
        <For each={rows()}>
          {() => (
            <div class="flex gap-4">
              <For each={columns()}>
                {() => <div class="h-4 flex-1 rounded-chip bg-hover motion-safe:animate-pulse" />}
              </For>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}

/** Failure of a list or data source: says what failed and offers one recovery. */
export function ListFailed(props: { message?: string; onRetry?: () => void }): JSX.Element {
  const site = useSite();
  return (
    <div class="flex flex-wrap items-center gap-3" role="alert" data-list-state="failed">
      <p class="m-0 text-small">{props.message || site.t("list.failed")}</p>
      {props.onRetry ? (
        <Button variant="secondary" size="sm" onClick={() => props.onRetry?.()}>
          {site.t("app.retry")}
        </Button>
      ) : null}
    </div>
  );
}
