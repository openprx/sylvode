import { useParams } from "@solidjs/router";
import type { JSX } from "@solidjs/web";
import { createSignal, onSettled, Show } from "solid-js";
import type { ListStatus } from "../components/list-state";
import { PageTitle } from "../components/page-title";
import { RecordTable } from "../components/record-table";
import { Button } from "../components/ui";
import { ManifestGate, useSite } from "../lib/site/store";
import type { Manifest, Paginated, SiteField, SiteRecord } from "../lib/site/types";

const PER_PAGE = 20;

function Mine(props: { binding: string; manifest: Manifest }): JSX.Element {
  const site = useSite();
  const info = props.manifest.bindings.find((item) => item.key === props.binding);
  const [fields, setFields] = createSignal<SiteField[]>([]);
  const [result, setResult] = createSignal<Paginated<SiteRecord> | null>(null);
  const [page, setPage] = createSignal(1);
  const [status, setStatus] = createSignal<ListStatus>("loading");

  const loadFields = async () => {
    if (fields().length > 0) return;
    const response = await site.client.fields(props.binding);
    const keep = new Set(info?.visitor.read_fields ?? []);
    setFields(() => response.fields.filter((field) => keep.has(field.key)));
  };

  const fetchPage = async (next: number) => {
    try {
      return await site.client.listRecords(props.binding, {
        page: next,
        per_page: PER_PAGE,
        sort: "-created_at",
      });
    } catch {
      // `created_at` may be absent from the visitor sort list: retry unsorted before failing.
      return await site.client.listRecords(props.binding, { page: next, per_page: PER_PAGE });
    }
  };

  /** Fields and one page of records; the list is `ready` only when both arrived. */
  const load = async (next: number) => {
    if (!result()) setStatus("loading");
    try {
      await loadFields();
      const data = await fetchPage(next);
      setResult(() => data);
      setPage(next);
      setStatus("ready");
    } catch {
      setStatus("failed");
    }
  };

  onSettled(() => {
    if (!info || info.visitor.read_fields.length === 0) return;
    void load(1);
  });

  const retry = () => {
    setResult(() => null);
    void load(page());
  };

  const pages = () => Math.max(Math.ceil((result()?.total ?? 0) / PER_PAGE), 1);

  return (
    <section class="flex flex-col gap-4" data-view="mine">
      <PageTitle title={site.t("mine.title")} />
      <h1 class="text-page font-semibold m-0">{site.t("mine.title")}</h1>
      <Show
        when={info && info.visitor.read_fields.length > 0}
        fallback={<p class="text-secondary m-0">{site.t("mine.none")}</p>}
      >
        <RecordTable
          fields={fields()}
          rows={result()?.items ?? []}
          locale={site.locale}
          empty={site.t("mine.empty")}
          status={status()}
          onRetry={retry}
        />
        <Show when={pages() > 1}>
          <div class="flex items-center gap-3">
            <Button
              variant="secondary"
              size="sm"
              disabled={page() <= 1}
              onClick={() => void load(page() - 1)}
            >
              {site.t("op.prev")}
            </Button>
            <span class="text-secondary text-small">
              {site.t("op.page", { page: page(), pages: pages() })}
            </span>
            <Button
              variant="secondary"
              size="sm"
              disabled={page() >= pages()}
              onClick={() => void load(page() + 1)}
            >
              {site.t("op.next")}
            </Button>
          </div>
        </Show>
      </Show>
    </section>
  );
}

/** The visitor's own records of one binding (`scope: own`; the server filters by the cookie). */
export default function MineRoute(): JSX.Element {
  const params = useParams();
  return (
    <ManifestGate>
      {(manifest) => (
        <Show when={params.binding} keyed>
          {(binding) => <Mine binding={binding} manifest={manifest} />}
        </Show>
      )}
    </ManifestGate>
  );
}
