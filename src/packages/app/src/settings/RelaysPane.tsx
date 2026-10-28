import { useState, type CSSProperties } from "react";
import { Button } from "@agent-hud/ui/components/button";
import { Input } from "@agent-hud/ui/components/input";
import { Switch } from "@agent-hud/ui/components/switch";
import { moneyAmount, snapshotLanguage } from "@agent-hud/ui/format";
import type { RelayCustom, RelayKind, RelaySource } from "@agent-hud/protocol";
import {
  Divider,
  Row,
  secondary,
  Section,
  useText,
  type SettingsPaneProps,
  type Text,
} from "./SettingsPrimitives";

/**
 * The relay page: every relay once, by its address, whether a client's
 * configuration names it or the owner added it by hand. Each row says which
 * agents use it (the clients configured with it, and the agents whose calls
 * went through it), what it has left, whether Agent HUD may read it, and what
 * kind of relay it is; the type is chosen by hand only when sniffing found
 * none.
 */
export function RelaysPane({ snapshot, action }: SettingsPaneProps) {
  const t = useText(snapshot);
  const language = snapshotLanguage(snapshot);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  // The relay address whose type is being chosen by hand.
  const [choosing, setChoosing] = useState<string | null>(null);
  const relays = snapshot.relays ?? [];
  const consented = snapshot.settings.relayReading ?? [];
  const agentName = (id: string) =>
    snapshot.providers.find((provider) => provider.id === id)?.name ?? id;
  const kinds: Record<string, string> = {
    newapi: "New API",
    sub2api: "Sub2API",
    openrouter: "OpenRouter",
    custom: t("Custom endpoint", "自定义接口"),
  };
  const perform = async (job: () => Promise<unknown>) => {
    setPending(true);
    setError(null);
    try {
      await job();
      return true;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
      return false;
    } finally {
      setPending(false);
    }
  };
  const describe = (relay: RelaySource) => {
    const users = relay.agents.length
      ? t(
          `Used by ${relay.agents.map(agentName).join(", ")}`,
          `${relay.agents.map(agentName).join("、")} 在用`,
        )
      : t("No agent uses it yet", "还没有智能体在用");
    const shown = relay.chosen ?? relay.kind;
    const kind = shown
      ? relay.chosen
        ? `${kinds[shown]}（${t("chosen", "手选")}）`
        : kinds[shown]
      : undefined;
    return [relay.name ? relay.provider : undefined, users, kind, relay.notice]
      .filter(Boolean)
      .join(" · ");
  };
  return (
    <div style={{ display: "grid", gap: 24 }}>
      <Section
        title={t("Relays", "中转站")}
        subtitle={t(
          "Found in Claude Code's and Codex's configuration, or added by hand. Agent HUD reads a relay's balance with the key already set up for it, and sends that key only to the relay's own address.",
          "从 Claude Code、Codex 的配置里找到，也可以手动添加。读取余额时用已经为它配置的 key，key 只发给这个中转站自己的地址。",
        )}
      >
        {relays.map((relay, index) => {
          const agreed = relay.manual || consented.includes(relay.provider);
          // The type is chosen by hand only when sniffing found none, or to
          // change a choice made before.
          const choosable =
            (relay.unrecognized || relay.chosen) && agreed && !pending;
          return (
            <div key={relay.provider}>
              {index > 0 && <Divider />}
              <Row
                title={relay.name ?? relay.provider}
                tag={relay.manual ? t("Added by hand", "手动添加") : undefined}
                subtitle={describe(relay)}
              >
                {relay.balance && (
                  <span
                    style={{ fontSize: 13, fontVariantNumeric: "tabular-nums" }}
                  >
                    {moneyAmount(
                      relay.balance.amount,
                      relay.balance.currency,
                      language,
                    )}
                  </span>
                )}
                {choosable && choosing !== relay.provider && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setChoosing(relay.provider)}
                  >
                    {t("Choose type", "手选类型")}
                  </Button>
                )}
                {relay.manual ? (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={pending}
                    onClick={() =>
                      void perform(() =>
                        action("remove_relay_source", { id: relay.id }),
                      )
                    }
                  >
                    {t("Remove", "移除")}
                  </Button>
                ) : (
                  <Switch
                    checked={agreed}
                    disabled={pending}
                    aria-label={t(
                      `Read ${relay.provider}'s balance`,
                      `读取 ${relay.provider} 的余额`,
                    )}
                    onCheckedChange={(value) =>
                      void perform(() =>
                        action("set_setting", {
                          key: "relayReading",
                          value: value
                            ? [...consented, relay.provider]
                            : consented.filter(
                                (host) => host !== relay.provider,
                              ),
                        }),
                      )
                    }
                  />
                )}
              </Row>
              {choosing === relay.provider && (
                <RelayKindForm
                  t={t}
                  pending={pending}
                  kind={relay.chosen ?? "auto"}
                  custom={relay.custom}
                  onCancel={() => setChoosing(null)}
                  onSave={(choice) =>
                    void perform(() =>
                      action("set_relay_kind", {
                        ...choice,
                        host: relay.provider,
                      }),
                    ).then((saved) => {
                      if (saved) setChoosing(null);
                    })
                  }
                />
              )}
            </div>
          );
        })}
        {relays.length === 0 && (
          <p
            style={{
              ...secondary,
              fontSize: 12,
              margin: 0,
              padding: "14px 16px",
            }}
          >
            {t(
              "No relay yet. One shows here when Claude Code or Codex is set up with one, or when you add it.",
              "还没有中转站。Claude Code 或 Codex 配置了中转站时会出现在这里，也可以手动添加。",
            )}
          </p>
        )}
        <Divider />
        {adding ? (
          <RelayForm
            t={t}
            pending={pending}
            onCancel={() => setAdding(false)}
            onSave={(relay) =>
              void perform(() => action("add_relay_source", relay)).then(
                (saved) => {
                  if (saved) setAdding(false);
                },
              )
            }
          />
        ) : (
          <div style={{ padding: "12px 16px" }}>
            <Button
              variant="outline"
              size="sm"
              disabled={pending}
              onClick={() => setAdding(true)}
            >
              {t("Add a relay", "添加中转站")}
            </Button>
          </div>
        )}
      </Section>
      {error && (
        <p style={{ color: "var(--destructive)", fontSize: 12, margin: 0 }}>
          {error}
        </p>
      )}
    </div>
  );
}

/** One labelled text field of the relay forms. */
function relayField(
  label: string,
  value: string,
  change: (value: string) => void,
  options: { secret?: boolean; placeholder?: string } = {},
) {
  return (
    <label style={{ display: "grid", gap: 4, fontSize: 12 }}>
      {label}
      <Input
        type={options.secret ? "password" : "text"}
        autoComplete="off"
        value={value}
        placeholder={options.placeholder}
        onChange={(event) => change(event.target.value)}
      />
    </label>
  );
}

/** The buttons under a relay form. */
function relayFormButtons(
  t: Text,
  pending: boolean,
  ready: boolean,
  save: string,
  onCancel: () => void,
) {
  return (
    <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={pending}
        onClick={onCancel}
      >
        {t("Cancel", "取消")}
      </Button>
      <Button type="submit" size="sm" disabled={pending || !ready}>
        {save}
      </Button>
    </div>
  );
}

/** A relay added by hand: its address and key, and a name if the owner wants
 * one. What kind of relay it is is found out when it is read. */
function RelayForm({
  t,
  pending,
  onSave,
  onCancel,
}: {
  t: Text;
  pending: boolean;
  onSave: (relay: Record<string, unknown>) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [baseUrl, setBaseUrl] = useState("");
  const [key, setKey] = useState("");
  return (
    <form
      style={{ display: "grid", gap: 10, padding: 16 }}
      onSubmit={(event) => {
        event.preventDefault();
        onSave({ name, baseUrl, key });
      }}
    >
      {relayField(t("Address", "地址"), baseUrl, setBaseUrl, {
        placeholder: "https://relay.example.com",
      })}
      {relayField(t("Key", "Key"), key, setKey, { secret: true })}
      {relayField(t("Name (optional)", "名称（可不填）"), name, setName)}
      <p style={{ fontSize: 11, color: "var(--muted-foreground)", margin: 0 }}>
        {t(
          "The key is kept privately on this Mac and sent only to this address.",
          "key 只保存在这台 Mac 上，只发给这个地址。",
        )}
      </p>
      {relayFormButtons(
        t,
        pending,
        Boolean(baseUrl.trim() && key.trim()),
        t("Add", "添加"),
        onCancel,
      )}
    </form>
  );
}

/** The type of a relay sniffing could not tell, chosen by hand for its
 * address: one of the common templates, or its own balance endpoint. */
function RelayKindForm({
  t,
  pending,
  kind: chosen,
  custom: saved,
  onSave,
  onCancel,
}: {
  t: Text;
  pending: boolean;
  kind: RelayKind;
  custom?: RelayCustom;
  onSave: (choice: { kind: RelayKind; custom?: RelayCustom }) => void;
  onCancel: () => void;
}) {
  const [kind, setKind] = useState<RelayKind>(chosen);
  const [url, setUrl] = useState(saved?.url ?? "");
  const [remaining, setRemaining] = useState(saved?.remaining ?? "");
  const [divisor, setDivisor] = useState(String(saved?.divisor ?? 1));
  const [currency, setCurrency] = useState<"USD" | "CNY">(
    saved?.currency ?? "USD",
  );
  const [header, setHeader] = useState(saved?.header ?? "");
  const kinds: [RelayKind, string][] = [
    ["auto", t("Find out", "自动识别")],
    ["newapi", "New API"],
    ["sub2api", "Sub2API"],
    ["openrouter", "OpenRouter"],
    ["custom", t("Custom endpoint", "自定义接口")],
  ];
  const select: CSSProperties = { height: 30, borderRadius: 6, fontSize: 12 };
  return (
    <form
      style={{ display: "grid", gap: 10, padding: "0 16px 16px" }}
      onSubmit={(event) => {
        event.preventDefault();
        onSave(
          kind === "custom"
            ? {
                kind,
                custom: {
                  url,
                  remaining,
                  divisor: Number(divisor) || 1,
                  currency,
                  header,
                },
              }
            : { kind },
        );
      }}
    >
      <label style={{ display: "grid", gap: 4, fontSize: 12 }}>
        {t("Type", "类型")}
        <select
          value={kind}
          onChange={(event) => setKind(event.target.value as RelayKind)}
          style={select}
        >
          {kinds.map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </label>
      {kind === "custom" && (
        <>
          {relayField(t("Balance endpoint", "余额接口"), url, setUrl, {
            placeholder: t(
              "/api/balance, on the relay's own address",
              "/api/balance，须在中转站自己的地址上",
            ),
          })}
          {relayField(
            t("Balance in the answer", "余额在返回里的位置"),
            remaining,
            setRemaining,
            { placeholder: "data.balance" },
          )}
          {relayField(t("Divide by", "除数"), divisor, setDivisor)}
          <label style={{ display: "grid", gap: 4, fontSize: 12 }}>
            {t("Currency", "币种")}
            <select
              value={currency}
              onChange={(event) =>
                setCurrency(event.target.value as "USD" | "CNY")
              }
              style={select}
            >
              <option value="USD">USD</option>
              <option value="CNY">CNY</option>
            </select>
          </label>
          {relayField(
            t("Key header (optional)", "key 放在哪个请求头（可不填）"),
            header,
            setHeader,
            { placeholder: "Authorization" },
          )}
        </>
      )}
      {relayFormButtons(
        t,
        pending,
        kind !== "custom" || Boolean(url.trim() && remaining.trim()),
        t("Save", "保存"),
        onCancel,
      )}
    </form>
  );
}
