import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { PageTitle } from "../components/page-title";
import { Button, Card } from "../components/ui";
import { ApiError } from "../lib/api";
import { useAppNavigate } from "../lib/base";
import { useSite } from "../lib/site/store";

const field =
  "w-full bg-input text-text text-body border border-input-border rounded-input px-3 h-9 focus-visible:outline-2 focus-visible:outline-focus";

/** Operator sign in (`POST /auth/operator/login`); the cookie `sv_site_op` is set by the server. */
export default function Login(): JSX.Element {
  const site = useSite();
  const navigate = useAppNavigate();
  const [email, setEmail] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await site.client.login(email(), password());
      setPassword("");
      await site.reload();
      const first = site.manifest()?.bindings.find((binding) => binding.operator);
      navigate(first ? `/operator/${first.key}` : "/");
    } catch (failure) {
      setError(
        failure instanceof ApiError && failure.status === 429
          ? site.t("login.rate")
          : site.t("login.failed"),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <section class="max-w-sm mx-auto flex flex-col gap-4">
      <PageTitle title={site.t("login.title")} />
      <h1 class="text-page font-semibold m-0">{site.t("login.title")}</h1>
      <Card>
        <form class="flex flex-col gap-3" onSubmit={submit}>
          <label class="flex flex-col gap-1">
            <span class="text-secondary font-medium">{site.t("login.email")}</span>
            <input
              class={field}
              type="email"
              name="email"
              autocomplete="username"
              required
              value={email()}
              onInput={(event) => setEmail(event.currentTarget.value)}
            />
          </label>
          <label class="flex flex-col gap-1">
            <span class="text-secondary font-medium">{site.t("login.password")}</span>
            <input
              class={field}
              type="password"
              name="password"
              autocomplete="current-password"
              required
              value={password()}
              onInput={(event) => setPassword(event.currentTarget.value)}
            />
          </label>
          <Show when={error()}>
            <p class="text-meta text-danger m-0" role="alert">
              {error()}
            </p>
          </Show>
          <Button type="submit" disabled={busy()}>
            {site.t("login.submit")}
          </Button>
        </form>
      </Card>
    </section>
  );
}
