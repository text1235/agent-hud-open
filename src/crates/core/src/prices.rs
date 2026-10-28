//! The price page (docs/proposals/no-plan-usage.md §3): every model the
//! selected logs used in the last 30 days, what it costs and where that price
//! comes from, and the relays and local hosts their calls went through, with
//! what the owner set for each.
use crate::{pricing, AgentHudCore};
use agent_hud_agents::endpoints::{host_kind, HostKind};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const DAYS_30: i64 = 30 * 86_400_000;

/// What kind of route a call went through, as the page groups them.
fn route_kind(route: &str) -> &'static str {
    if pricing::is_local_route(route) {
        "local-server"
    } else if matches!(route, "bedrock" | "vertex" | "foundry") {
        "cloud"
    } else if route.contains('.') || route.contains(':') {
        match host_kind(route) {
            HostKind::Local => "local",
            _ => "relay",
        }
    } else {
        "provider"
    }
}

impl AgentHudCore {
    pub(crate) fn price_list(&mut self) -> Result<Value, String> {
        self.ensure_usage_projection();
        self.prepare_meters()?;
        let now = chrono::Utc::now().timestamp_millis();
        let Some(view) = self.meters.view.as_ref() else {
            return Ok(json!({"kind":"prices","models":[],"relays":[],"localHosts":[]}));
        };
        let settings = &self.settings;
        let mut relays = BTreeMap::<String, Value>::new();
        let mut locals = BTreeMap::<String, Value>::new();
        let models = view
            .models_since(now - DAYS_30)
            .into_iter()
            .filter(|(_, tokens)| *tokens > 0)
            .map(|((source, model), tokens)| {
                let id = format!("{source}-model:{model}");
                let route = model.split_once('#').map(|(_, route)| route);
                let kind = route.map(route_kind);
                let quote = pricing::price_quote(&id);
                if let (Some(route), Some(kind)) = (route, kind) {
                    if kind == "relay" {
                        relays.entry(route.to_owned()).or_insert_with(|| {
                            json!({"host": route, "multiplier": settings["relayMultipliers"][route],
                                   "listed": quote.as_ref().is_some_and(|(origin, _, _)| *origin == pricing::Origin::Relay)})
                        });
                    }
                    if kind == "local" {
                        locals
                            .entry(route.to_owned())
                            .or_insert_with(|| json!({"host": route, "kind": settings["localKinds"][route]}));
                    }
                }
                json!({
                    "id": id,
                    "source": source,
                    "client": agent_hud_agents::by_id(&source).map_or(source.as_str(), |agent| agent.name),
                    "model": model,
                    "name": crate::naming::model_names::consumer_name(&id),
                    "route": route,
                    "routeKind": kind,
                    "tokens": tokens,
                    "price": quote.map(|(origin, currency, rates)| json!({
                        "origin": origin.name(),
                        "currency": currency,
                        "input": rates[0],
                        "output": rates[1],
                        "cacheRead": rates[2],
                        "cacheWrite": rates[3],
                    })),
                    "custom": settings["customPrices"][model.as_str()],
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "kind": "prices",
            "models": models,
            "relays": relays.into_values().collect::<Vec<_>>(),
            "localHosts": locals.into_values().collect::<Vec<_>>(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use std::fs;

    #[test]
    fn the_price_page_lists_used_models_and_follows_what_the_owner_sets() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".claude/projects/p");
        fs::create_dir_all(&root).unwrap();
        let id = "22222222-3333-4444-5555-666666666666";
        let at = |offset: i64| {
            DateTime::<Utc>::from_timestamp_millis(
                Utc::now().timestamp_millis() - 3_600_000 + offset,
            )
            .unwrap()
            .to_rfc3339()
        };
        let reply = |offset: i64, message: &str, model: &str| json!({"type":"assistant","sessionId":id,"timestamp":at(offset),"message":{"id":message,"model":model,"role":"assistant","content":[{"type":"text","text":"ok"}],"usage":{"input_tokens":1000,"output_tokens":100,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}});
        let mut core = AgentHudCore::new(vec![], home.join("app"));
        core.provider_roots = vec![(home.join(".claude/projects"), "claude")];
        core.request("snapshot", json!({})).unwrap();
        // The session runs through a local address no model server answered at.
        core.ingest_hook_events(vec![agent_hud_agents::HookEvent {
            source: "claude".into(),
            session_id: id.into(),
            at_ms: Utc::now().timestamp_millis(),
            kind: "origin".into(),
            data: json!({"endpoint": {"host": "localhost:4000", "platform": null}}),
        }]);
        let lines = [
            json!({"type":"user","sessionId":id,"timestamp":at(0),"message":{"role":"user","content":"hi"}}),
            reply(1_000, "m1", "claude-opus-4-5"),
            reply(2_000, "m2", "qwen3-coder:30b"),
        ];
        fs::write(
            root.join(format!("{id}.jsonl")),
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        core.request("refresh", json!({})).unwrap();
        let list = core.request("price_list", json!({})).unwrap();
        let row = |list: &Value, model: &str| {
            list["models"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["model"] == model)
                .cloned()
                .unwrap_or_else(|| panic!("{model} in {list:#}"))
        };
        assert_eq!(
            row(&list, "claude-opus-4-5#localhost:4000")["price"],
            Value::Null,
            "a listed model goes through a gateway"
        );
        assert_eq!(
            row(&list, "qwen3-coder:30b#localhost:4000")["price"]["origin"],
            "local"
        );
        assert_eq!(
            list["localHosts"],
            json!([{"host": "localhost:4000", "kind": null}])
        );
        assert_eq!(
            row(&list, "qwen3-coder:30b#localhost:4000")["routeKind"],
            "local"
        );

        // The owner calls the address a model server, then prices the gateway's model.
        core.request(
            "set_setting",
            json!({"key": "localKinds", "value": {"localhost:4000": "model"}}),
        )
        .unwrap();
        let list = core.request("price_list", json!({})).unwrap();
        assert_eq!(
            row(&list, "claude-opus-4-5#localhost:4000")["price"]["origin"],
            "local"
        );
        core.request(
            "set_setting",
            json!({"key": "localKinds", "value": {"localhost:4000": "gateway"}}),
        )
        .unwrap();
        core.request(
            "set_setting",
            json!({"key": "customPrices", "value": {"claude-opus-4-5#localhost:4000": {"currency": "USD", "input": 2.0, "output": 10.0, "cacheRead": null, "cacheWrite": null}}}),
        )
        .unwrap();
        let list = core.request("price_list", json!({})).unwrap();
        let opus = row(&list, "claude-opus-4-5#localhost:4000");
        assert_eq!(opus["price"]["origin"], "custom");
        assert_eq!(opus["price"]["input"], 2.0);
        assert_eq!(opus["custom"]["output"], 10.0);
        assert_eq!(
            row(&list, "qwen3-coder:30b#localhost:4000")["price"],
            Value::Null,
            "a gateway: not free any more"
        );
        assert!(core
            .request(
                "set_setting",
                json!({"key": "relayMultipliers", "value": {"x.example.com": 0}})
            )
            .is_err());
    }

    #[test]
    fn routes_are_grouped_by_what_they_reach() {
        assert_eq!(route_kind("ollama"), "local-server");
        assert_eq!(route_kind("bedrock"), "cloud");
        assert_eq!(route_kind("relay.example.com"), "relay");
        assert_eq!(route_kind("localhost:4000"), "local");
        assert_eq!(route_kind("192.168.1.9:8000"), "local");
        assert_eq!(route_kind("openrouter"), "provider");
    }
}
