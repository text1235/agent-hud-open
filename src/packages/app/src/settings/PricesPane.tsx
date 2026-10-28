import { useCallback, useEffect, useState } from "react";
import { Button } from "@agent-hud/ui/components/button";
import { Input } from "@agent-hud/ui/components/input";
import {
  moneyAmount,
  snapshotLanguage,
  tokensShort,
} from "@agent-hud/ui/format";
import type { CustomPrice, PriceList, PriceRow } from "@agent-hud/protocol";
import {
  Divider,
  Row,
  secondary,
  Section,
  Segments,
  useText,
  type SettingsPaneProps,
} from "./SettingsPrimitives";

type Rates = Record<"input" | "output" | "cacheRead" | "cacheWrite", string>;
const fields = ["input", "output", "cacheRead", "cacheWrite"] as const;

/**
 * The price page: every model used in the last 30 days, what it costs per
 * million tokens and where that comes from (the vendor's list, a relay's own
 * list, a multiplier, the owner's price, or free on this computer), with the
 * owner's own price, each relay's multiplier and what each local host is as
 * the fallbacks when the automatic price is wrong or missing.
 */
export function PricesPane({ snapshot, action }: SettingsPaneProps) {
  const t = useText(snapshot);
  const language = snapshotLanguage(snapshot);
  const [list, setList] = useState<PriceList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const load = useCallback(
    () =>
      action("price_list")
        .then((answer) => setList(answer as PriceList))
        .catch((failure: unknown) =>
          setError(
            failure instanceof Error ? failure.message : String(failure),
          ),
        ),
    [action],
  );
  useEffect(() => {
    void load();
  }, [load]);
  const save = async (key: string, value: unknown) => {
    setPending(true);
    setError(null);
    try {
      await action("set_setting", { key, value });
      await load();
      return true;
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
      return false;
    } finally {
      setPending(false);
    }
  };
  const customPrices = snapshot.settings.customPrices ?? {};
  const multipliers = snapshot.settings.relayMultipliers ?? {};
  const localKinds = snapshot.settings.localKinds ?? {};
  const origins: Record<string, string> = {
    catalog: t("List price", "价目表"),
    relay: t("Relay's list", "中转站价格表"),
    multiplier: t("Multiplier", "倍率"),
    custom: t("Your price", "自定义"),
    local: t("Local", "本地"),
  };
  const money = (value: number, currency: string) =>
    moneyAmount(value, currency, language, true);
  const priceText = (row: PriceRow) => {
    if (!row.price) return t("Unpriced", "未计价");
    if (row.price.origin === "local") return t("Free", "免费");
    return t(
      `${money(row.price.input, row.price.currency)} in · ${money(row.price.output, row.price.currency)} out`,
      `输入 ${money(row.price.input, row.price.currency)} · 输出 ${money(row.price.output, row.price.currency)}`,
    );
  };
  const clients = [...new Set((list?.models ?? []).map((row) => row.client))];
  return (
    <div
      className="source-settings-pane"
      style={{
        width: "100%",
        maxWidth: 640,
        display: "flex",
        flexDirection: "column",
        gap: 28,
      }}
    >
      <p style={{ ...secondary, margin: 0, fontSize: 12 }}>
        {t(
          "What each model used in the last 30 days costs per million tokens, and where the price comes from. Change it here when the automatic price is wrong or missing; costs follow at once.",
          "最近 30 天用过的每个模型，每百万 Token 按什么价格算、价格从哪里来。自动得到的价格不对或没有时，可以在这里改，花费随即按新价格重算。",
        )}
      </p>
      {error && (
        <p style={{ margin: 0, fontSize: 12, color: "var(--destructive)" }}>
          {error}
        </p>
      )}
      {list && list.models.length === 0 && (
        <p style={{ ...secondary, margin: 0, fontSize: 12 }}>
          {t(
            "No model was used in the last 30 days.",
            "最近 30 天没有用过模型。",
          )}
        </p>
      )}
      {clients.map((client) => {
        const rows = (list?.models ?? []).filter(
          (row) => row.client === client,
        );
        return (
          <Section key={client} title={client}>
            {rows.map((row, index) => (
              <div key={row.id}>
                {index > 0 && <Divider />}
                <Row
                  title={row.name}
                  subtitle={`${tokensShort(row.tokens)} Token · ${priceText(row)}`}
                  tag={
                    row.price
                      ? origins[row.price.origin]
                      : t("Unpriced", "未计价")
                  }
                >
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={pending}
                    onClick={() =>
                      setEditing(editing === row.id ? null : row.id)
                    }
                  >
                    {t("Set price", "设置价格")}
                  </Button>
                </Row>
                {editing === row.id && (
                  <PriceEditor
                    row={row}
                    custom={customPrices[row.model] ?? null}
                    pending={pending}
                    t={t}
                    onSave={async (price) => {
                      const next = { ...customPrices };
                      if (price) next[row.model] = price;
                      else delete next[row.model];
                      if (await save("customPrices", next)) setEditing(null);
                    }}
                    onCancel={() => setEditing(null)}
                  />
                )}
              </div>
            ))}
          </Section>
        );
      })}
      {(list?.relays.length ?? 0) > 0 && (
        <Section
          title={t("Relays", "中转站")}
          subtitle={t(
            "A relay's own price list comes first. Without one, its calls cost the vendor's list price times this multiplier; leave it empty to keep them unpriced.",
            "优先用中转站自己的价格表。没有价格表时，走它的调用按官方价 × 这个倍率计算；留空则不计价。",
          )}
        >
          {list!.relays.map((relay, index) => (
            <div key={relay.host}>
              {index > 0 && <Divider />}
              <Row
                title={relay.host}
                subtitle={
                  relay.listed
                    ? t(
                        "Priced by the relay's own list",
                        "按中转站自己的价格表计价",
                      )
                    : t("No price list read from it", "没有读到它的价格表")
                }
              >
                <MultiplierInput
                  value={multipliers[relay.host]}
                  disabled={pending}
                  placeholder={t("e.g. 0.3", "如 0.3")}
                  onCommit={(times) => {
                    const next = { ...multipliers };
                    if (times === null) delete next[relay.host];
                    else next[relay.host] = times;
                    void save("relayMultipliers", next);
                  }}
                />
              </Row>
            </div>
          ))}
        </Section>
      )}
      {(list?.localHosts.length ?? 0) > 0 && (
        <Section
          title={t("Addresses on this computer", "本机地址")}
          subtitle={t(
            "Calls to a model server here are free; a gateway passes them on to a paid API. Automatic: a model with a list price goes through a gateway, any other runs here.",
            "本机上的模型服务按免费计算；本地网关会把调用转给付费 API。自动：价目表里有的模型按网关算，其他按本地模型算。",
          )}
        >
          {list!.localHosts.map((local, index) => (
            <div key={local.host}>
              {index > 0 && <Divider />}
              <Row title={local.host}>
                <Segments
                  label={local.host}
                  value={localKinds[local.host] ?? "auto"}
                  disabled={pending}
                  options={[
                    ["auto", t("Automatic", "自动")],
                    ["model", t("Model server", "本地模型")],
                    ["gateway", t("Gateway", "本地网关")],
                  ]}
                  change={(kind) => {
                    const next = { ...localKinds };
                    if (kind === "auto") delete next[local.host];
                    else next[local.host] = kind as "model" | "gateway";
                    void save("localKinds", next);
                  }}
                />
              </Row>
            </div>
          ))}
        </Section>
      )}
    </div>
  );
}

/** One model's own price per million tokens: empty means unknown, 0 free. */
function PriceEditor({
  row,
  custom,
  pending,
  t,
  onSave,
  onCancel,
}: {
  row: PriceRow;
  custom: CustomPrice | null;
  pending: boolean;
  t: (en: string, zh: string) => string;
  onSave: (price: CustomPrice | null) => void;
  onCancel: () => void;
}) {
  const start = custom ?? row.price;
  const [currency, setCurrency] = useState<"USD" | "CNY">(
    start?.currency === "CNY" ? "CNY" : "USD",
  );
  const [rates, setRates] = useState<Rates>(() => {
    const value = (key: (typeof fields)[number]) => {
      const number = start?.[key];
      return typeof number === "number"
        ? String(Number(number.toFixed(6)))
        : "";
    };
    return {
      input: value("input"),
      output: value("output"),
      cacheRead: value("cacheRead"),
      cacheWrite: value("cacheWrite"),
    };
  });
  const parsed = (text: string) => {
    if (text.trim() === "") return null;
    const number = Number(text);
    return Number.isFinite(number) && number >= 0 ? number : undefined;
  };
  const values = fields.map((key) => parsed(rates[key]));
  const valid =
    values.every((value) => value !== undefined) &&
    values[0] !== null &&
    values[1] !== null;
  const names: Record<(typeof fields)[number], string> = {
    input: t("Input", "输入"),
    output: t("Output", "输出"),
    cacheRead: t("Cache read", "缓存读"),
    cacheWrite: t("Cache write", "缓存写"),
  };
  return (
    <div style={{ display: "grid", gap: 10, padding: "4px 16px 14px" }}>
      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
        <span style={{ ...secondary, fontSize: 11 }}>
          {t("Per million tokens", "每百万 Token")}
        </span>
        <Segments
          label={t("Currency", "币种")}
          value={currency}
          options={[
            ["USD", "USD"],
            ["CNY", "CNY"],
          ]}
          change={(value) => setCurrency(value as "USD" | "CNY")}
        />
      </div>
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(4, minmax(0, 1fr))",
          gap: 8,
        }}
      >
        {fields.map((key) => (
          <label key={key} style={{ display: "grid", gap: 4, fontSize: 11 }}>
            <span style={secondary}>{names[key]}</span>
            <Input
              inputMode="decimal"
              value={rates[key]}
              placeholder={
                key.startsWith("cache") ? t("= input", "同输入") : ""
              }
              onChange={(event) =>
                setRates({ ...rates, [key]: event.target.value })
              }
            />
          </label>
        ))}
      </div>
      <p style={{ ...secondary, margin: 0, fontSize: 11 }}>
        {t(
          `Only for ${row.name}. 0 means free; an empty cache rate takes the input's.`,
          `只用于 ${row.name}。0 表示免费；缓存价留空时按输入价算。`,
        )}
      </p>
      <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
        {custom && (
          <Button
            variant="ghost"
            size="sm"
            disabled={pending}
            onClick={() => onSave(null)}
          >
            {t("Use the automatic price", "改回自动价格")}
          </Button>
        )}
        <Button variant="ghost" size="sm" disabled={pending} onClick={onCancel}>
          {t("Cancel", "取消")}
        </Button>
        <Button
          size="sm"
          disabled={pending || !valid}
          onClick={() =>
            onSave({
              currency,
              input: values[0] as number,
              output: values[1] as number,
              cacheRead: values[2] ?? null,
              cacheWrite: values[3] ?? null,
            })
          }
        >
          {t("Save", "保存")}
        </Button>
      </div>
    </div>
  );
}

/** A relay's multiplier, kept when the field is left or Return is pressed. */
function MultiplierInput({
  value,
  disabled,
  placeholder,
  onCommit,
}: {
  value: number | undefined;
  disabled: boolean;
  placeholder: string;
  onCommit: (times: number | null) => void;
}) {
  const [text, setText] = useState(value === undefined ? "" : String(value));
  useEffect(() => setText(value === undefined ? "" : String(value)), [value]);
  const commit = () => {
    const trimmed = text.trim();
    if (trimmed === "") {
      if (value !== undefined) onCommit(null);
      return;
    }
    const times = Number(trimmed);
    if (Number.isFinite(times) && times > 0 && times <= 100 && times !== value)
      onCommit(times);
    else if (!(Number.isFinite(times) && times > 0 && times <= 100))
      setText(value === undefined ? "" : String(value));
  };
  return (
    <Input
      inputMode="decimal"
      value={text}
      disabled={disabled}
      placeholder={placeholder}
      style={{ width: 84 }}
      onChange={(event) => setText(event.target.value)}
      onBlur={commit}
      onKeyDown={(event) => {
        if (event.key === "Enter") commit();
      }}
    />
  );
}
