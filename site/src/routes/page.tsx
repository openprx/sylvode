import { useLocation, useNavigate, useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createSignal, Match, onSettled, Show, Switch } from "solid-js";
import { LayoutView } from "../components/layout/layout-view";
import { ApiError } from "../lib/api";
import { stripBase, withBase } from "../lib/base";
import { createRuntimeEnv, matchPage } from "../lib/site/env";
import { ManifestGate, useSite } from "../lib/site/store";
import type { Manifest, PageResponse } from "../lib/site/types";
import NotFound from "./not-found";

type PageState =
  | { kind: "loading" }
  | { kind: "ready"; page: PageResponse }
  | { kind: "error"; status: number; code: string | undefined };

function PageLoader(props: { route: string; manifest: Manifest }): JSX.Element {
  const site = useSite();
  const navigate = useNavigate();
  const match = matchPage(props.manifest, props.route);
  const [state, setState] = createSignal<PageState>({ kind: "loading" });

  onSettled(() => {
    if (!match) return;
    site.client
      .page(props.route)
      .then((page) => setState(() => ({ kind: "ready", page })))
      .catch((error: unknown) =>
        setState(() =>
          error instanceof ApiError
            ? { kind: "error", status: error.status, code: error.errorCode }
            : { kind: "error", status: 0, code: undefined },
        ),
      );
  });

  const ready = () => {
    const current = state();
    return current.kind === "ready" ? current.page : undefined;
  };
  const errored = () => {
    const current = state();
    return current.kind === "error" ? current : undefined;
  };

  return (
    <Show when={match} fallback={<NotFound />}>
      <Switch>
        <Match when={state().kind === "loading"}>
          <p class="text-secondary">{site.t("app.loading")}</p>
        </Match>
        <Match when={errored()?.code === "operator_required" || errored()?.status === 401}>
          <section class="flex flex-col gap-3">
            <p class="m-0">{site.t("page.operatorRequired")}</p>
            <p class="m-0">
              <a href={withBase("/login")} class="text-text underline">
                {site.t("page.signIn")}
              </a>
            </p>
          </section>
        </Match>
        <Match when={errored()?.status === 404}>
          <NotFound />
        </Match>
        <Match when={errored()}>
          <p class="m-0" role="alert">
            {site.t("app.error")}
          </p>
        </Match>
        <Match when={ready()}>
          {(page) => (
            <LayoutView
              spec={page().layout}
              params={{ ...match?.params, ...page().params }}
              env={createRuntimeEnv(site, props.manifest, (path) => navigate(path))}
            />
          )}
        </Match>
      </Switch>
    </Show>
  );
}

/** Any manifest route; `/p/:slug` addresses the single-segment route `/<slug>`. */
export default function SitePage(): JSX.Element {
  const location = useLocation();
  const params = useParams();
  const route = () => (params.slug ? `/${params.slug}` : stripBase(location.pathname));
  return (
    <ManifestGate>
      {(manifest) => (
        <Show when={route()} keyed>
          {(current) => <PageLoader route={current} manifest={manifest} />}
        </Show>
      )}
    </ManifestGate>
  );
}
