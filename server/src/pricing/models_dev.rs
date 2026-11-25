//! Chooses one published price per model ID from the models.dev catalog.
use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;

use crate::{model::DefaultPrice, Error, Result};

/// Reseller consulted when a model's lab does not sell it directly.
const RESELLER: &str = "openrouter";

#[derive(Deserialize)]
struct Provider {
    models: BTreeMap<String, Model>,
}

#[derive(Deserialize)]
struct Model {
    canonical_model_id: Option<String>,
    cost: Option<Cost>,
}

#[derive(Deserialize)]
struct Cost {
    input: Option<f64>,
    output: Option<f64>,
    cache_read: Option<f64>,
    cache_write: Option<f64>,
}

/// A usable price for one provider's listing of an attributed model.
struct Listing {
    id: String,
    price: DefaultPrice,
}

pub(super) struct PriceCatalog(HashMap<String, DefaultPrice>);

impl PriceCatalog {
    pub fn parse(json: &[u8]) -> Result<Self> {
        let providers: BTreeMap<String, Provider> = serde_json::from_slice(json)?;
        // models.dev only attributes a model to a canonical ID when several
        // providers sell it. A lab's own catalogue routinely leaves the field
        // off (`gpt-4o`, `o3`, `claude-opus-4-6`, most Qwen models), so the
        // attributed pass alone would drop the official rate and fall back to a
        // reseller or to no price at all.
        //
        // A provider is a lab when it attributes a usable model to its own
        // namespace (`openai` → `openai/gpt-5`). Resellers, gateways, and
        // subscription plans attribute models to other labs, so their
        // unattributed listings still cannot price a bare ID. A provider whose
        // only self-attributed entries are unusable stays a relay as well.
        let labs = providers
            .iter()
            .filter(|(provider, Provider { models })| {
                models.values().any(|model| {
                    model
                        .canonical_model_id
                        .as_deref()
                        .is_some_and(|canonical| lab(canonical) == provider.as_str())
                        && model
                            .cost
                            .as_ref()
                            .and_then(|cost| cost.price(provider))
                            .is_some()
                })
            })
            .map(|(provider, _)| provider.as_str())
            .collect::<HashSet<_>>();
        let mut votes = HashMap::<String, BTreeMap<String, usize>>::new();
        let mut listings = HashMap::<(String, String), Vec<Listing>>::new();
        let mut prices = HashMap::new();
        for (provider, Provider { models }) in &providers {
            for (id, model) in models {
                let Some(price) = model.cost.as_ref().and_then(|cost| cost.price(provider)) else {
                    continue;
                };
                let id = model_key(id);
                let Some(canonical) = model.canonical_model_id.as_deref() else {
                    // An unattributed listing from a lab names that lab's own
                    // model, so it can satisfy a vote another provider cast.
                    if !labs.contains(provider.as_str()) {
                        continue;
                    }
                    let own = format!("{provider}/{id}");
                    listings
                        .entry((provider.clone(), own))
                        .or_default()
                        .push(Listing {
                            id: id.clone(),
                            price: price.clone(),
                        });
                    prices.entry(id).or_insert(price);
                    continue;
                };
                *votes
                    .entry(id.clone())
                    .or_default()
                    .entry(canonical.to_string())
                    .or_default() += 1;
                if lab(canonical) == provider {
                    prices.entry(id.clone()).or_insert_with(|| price.clone());
                }
                listings
                    .entry((provider.clone(), canonical.to_string()))
                    .or_default()
                    .push(Listing { id, price });
            }
        }
        for (id, votes) in votes {
            // Resellers sometimes attribute an ID differently, so the majority decides.
            let Some((canonical, _)) = votes.into_iter().rev().max_by_key(|(_, count)| *count)
            else {
                continue;
            };
            if prices.contains_key(&id) {
                continue;
            }
            let published = |provider: &str, paid: bool| {
                closest(
                    listings.get(&(provider.to_string(), canonical.clone()))?,
                    &model_key(&canonical),
                    paid,
                )
            };
            // A reseller's zero is a promotion or a subscription, not a price.
            if let Some(price) =
                published(lab(&canonical), false).or_else(|| published(RESELLER, true))
            {
                prices.insert(id, price);
            }
        }
        if prices.is_empty() {
            return Err(Error::Provider(
                "models.dev catalog contains no model prices".into(),
            ));
        }
        Ok(Self(prices))
    }

    pub fn price(&self, model_id: &str) -> Option<DefaultPrice> {
        self.0.get(&model_key(model_id)).cloned()
    }
}

impl Cost {
    fn price(&self, source: &str) -> Option<DefaultPrice> {
        let (input, output) = (self.input?, self.output?);
        [Some(input), Some(output), self.cache_read, self.cache_write]
            .into_iter()
            .flatten()
            .all(|rate| rate >= 0.0)
            .then(|| DefaultPrice {
                input_per_million: input,
                output_per_million: output,
                cache_read_per_million: self.cache_read,
                cache_write_per_million: self.cache_write,
                source: source.into(),
            })
    }
}

/// Gateways prefix IDs with a vendor path and vary their letter case.
fn model_key(id: &str) -> String {
    id.rsplit('/').next().unwrap_or(id).to_lowercase()
}

fn lab(canonical: &str) -> &str {
    canonical.split('/').next().unwrap_or(canonical)
}

fn closest(listings: &[Listing], name: &str, paid: bool) -> Option<DefaultPrice> {
    listings
        .iter()
        .filter(|Listing { price, .. }| {
            !paid || price.input_per_million + price.output_per_million > 0.0
        })
        .min_by_key(|Listing { id, .. }| (id != name, id.len(), id))
        .map(|listing| listing.price.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn model(canonical: &str, cost: Value) -> Value {
        json!({"canonical_model_id": canonical, "cost": cost})
    }

    fn catalog() -> PriceCatalog {
        let catalog = json!({
            "zhipuai": {"models": {
                "glm-5.3": model("zhipuai/glm-5.3", json!({"input": 1.2, "output": 4.2, "cache_read": 0.3, "cache_write": 0})),
                "glm-5.3-flash": model("zhipuai/glm-5.3-flash", json!({"input": 0.1, "output": 0.4})),
                "glm-5.3-flashx": model("zhipuai/glm-5.3-flash", json!({"input": 0.3, "output": 1.0})),
                "glm-free": model("zhipuai/glm-free", json!({"input": 0, "output": 0})),
            }},
            "zhipuai-coding-plan": {"models": {
                "glm-5.3": model("zhipuai/glm-5.3", json!({"input": 0, "output": 0})),
                "glm-5.4": model("zhipuai/glm-5.4", json!({"input": 0, "output": 0})),
            }},
            "relay": {"models": {
                "vendor/GLM-5.3": model("zhipuai/glm-5.3", json!({"input": 9, "output": 9})),
                "GLM-5.3-fp8": model("zhipuai/glm-5.3", json!({"input": 8, "output": 8})),
                "open-model": model("small-lab/open-model", json!({"input": 7, "output": 7})),
                "unattributed": {"cost": {"input": 1, "output": 1}},
                "broken": model("relay/broken", json!({"input": -1, "output": 1})),
                "half": model("relay/half", json!({"input": 1})),
                "image": {"name": "no cost"},
            }},
            "openrouter": {"models": {
                "small-lab/open-model": model("small-lab/open-model", json!({"input": 0.5, "output": 1.5})),
                "small-lab/open-model:free": model("small-lab/open-model", json!({"input": 0, "output": 0})),
                "z-ai/glm-5.3": model("zhipuai/glm-5.3", json!({"input": 0.07, "output": 1.4})),
            }},
        });
        PriceCatalog::parse(catalog.to_string().as_bytes()).unwrap()
    }

    fn found(id: &str) -> Option<(f64, f64, String)> {
        catalog().price(id).map(|price| {
            (
                price.input_per_million,
                price.output_per_million,
                price.source,
            )
        })
    }

    fn priced(input: f64, output: f64, source: &str) -> Option<(f64, f64, String)> {
        Some((input, output, source.into()))
    }

    #[test]
    fn an_unattributed_lab_listing_is_priced_from_its_own_provider() {
        // models.dev leaves `canonical_model_id` off many models a lab sells
        // directly — gpt-4o, gpt-4.1, o3, claude-opus-4-6 — so skipping them
        // dropped the official price and fell back to a reseller, or to no
        // price at all. `openai` is corroborated as a lab namespace by the
        // reseller's attribution.
        let catalog = json!({
            "openai": {"models": {
                "gpt-5": model("openai/gpt-5", json!({"input": 1.25, "output": 10})),
                "gpt-4o": {"cost": {"input": 2.5, "output": 10, "cache_read": 1.25}},
                "o3": {"cost": {"input": 2, "output": 8, "cache_read": 0.5}},
            }},
            "openrouter": {"models": {
                "openai/gpt-4o": model("openai/gpt-4o", json!({"input": 9, "output": 9})),
            }},
        });
        let catalog = PriceCatalog::parse(catalog.to_string().as_bytes()).unwrap();
        let gpt = catalog.price("gpt-4o").unwrap();
        assert_eq!(
            (
                gpt.input_per_million,
                gpt.output_per_million,
                gpt.source.as_str()
            ),
            (2.5, 10.0, "openai"),
            "the lab's own listing must win over the reseller"
        );
        assert_eq!(catalog.price("o3").unwrap().input_per_million, 2.0);
    }

    #[test]
    fn a_relay_listing_without_attribution_is_not_treated_as_a_lab() {
        // A relay is never a lab namespace: every model it sells is attributed
        // to some other lab, so its unattributed entry must not become a price
        // for the bare ID — nothing ties it to the model the user called.
        let catalog = json!({
            "zhipuai": {"models": {
                "glm-5.3": model("zhipuai/glm-5.3", json!({"input": 1.2, "output": 4.2})),
            }},
            "relay": {"models": {
                "vendor/glm-5.3": model("zhipuai/glm-5.3", json!({"input": 9, "output": 9})),
                "unattributed": {"cost": {"input": 1, "output": 1}},
            }},
        });
        let catalog = PriceCatalog::parse(catalog.to_string().as_bytes()).unwrap();
        assert_eq!(catalog.price("unattributed"), None);
        // The relay's attributed listing still resolves through the lab price.
        assert_eq!(catalog.price("glm-5.3").unwrap().source, "zhipuai");
    }

    #[test]
    fn a_lab_listing_does_not_override_an_attributed_price() {
        // The attributed pass runs first and is authoritative.
        let catalog = json!({
            "openai": {"models": {
                "gpt-5": model("openai/gpt-5", json!({"input": 1.25, "output": 10})),
            }},
            "relay": {"models": {
                "gpt-5": {"cost": {"input": 99, "output": 99}},
            }},
        });
        let catalog = PriceCatalog::parse(catalog.to_string().as_bytes()).unwrap();
        let price = catalog.price("gpt-5").unwrap();
        assert_eq!(
            (price.input_per_million, price.source.as_str()),
            (1.25, "openai")
        );
    }

    #[test]
    fn the_labs_own_price_wins_over_resellers_and_subscriptions() {
        assert_eq!(found("glm-5.3"), priced(1.2, 4.2, "zhipuai"));
        let price = catalog().price("glm-5.3").unwrap();
        assert_eq!(price.cache_read_per_million, Some(0.3));
        assert_eq!(price.cache_write_per_million, Some(0.0));
    }

    #[test]
    fn gateway_prefixes_and_letter_case_do_not_hide_a_model() {
        assert_eq!(found("zcode/GLM-5.3"), priced(1.2, 4.2, "zhipuai"));
    }

    #[test]
    fn a_variant_keeps_its_own_price() {
        assert_eq!(found("glm-5.3-flashx"), priced(0.3, 1.0, "zhipuai"));
        assert_eq!(found("glm-5.3-flash"), priced(0.1, 0.4, "zhipuai"));
    }

    #[test]
    fn a_resellers_name_for_a_model_gets_the_labs_price() {
        assert_eq!(found("glm-5.3-fp8"), priced(1.2, 4.2, "zhipuai"));
    }

    #[test]
    fn the_majority_of_providers_decides_which_model_an_id_names() {
        let listing = |canonical: &str| json!({"models": {"shared": model(canonical, json!({"input": 9, "output": 9}))}});
        let lab = |id: &str, rate: f64| json!({"models": {id: model(&format!("lab/{id}"), json!({"input": rate, "output": rate}))}});
        let catalog = json!({
            "lab": {"models": {
                "new": lab("new", 2.0)["models"]["new"],
                "old": lab("old", 1.0)["models"]["old"],
            }},
            "relay-a": listing("lab/old"),
            "relay-b": listing("lab/new"),
            "relay-c": listing("lab/new"),
        });
        let catalog = PriceCatalog::parse(catalog.to_string().as_bytes()).unwrap();
        assert_eq!(catalog.price("shared").unwrap().input_per_million, 2.0);
    }

    #[test]
    fn a_lab_without_its_own_api_uses_the_paid_openrouter_price() {
        assert_eq!(found("open-model"), priced(0.5, 1.5, "openrouter"));
    }

    #[test]
    fn a_lab_may_publish_a_free_model() {
        assert_eq!(found("glm-free"), priced(0.0, 0.0, "zhipuai"));
    }

    #[test]
    fn models_without_a_trustworthy_price_stay_unpriced() {
        for id in [
            "glm-5.4",
            "unattributed",
            "broken",
            "half",
            "image",
            "never-heard-of-it",
            "",
        ] {
            assert_eq!(found(id), None, "{id}");
        }
    }

    #[test]
    fn an_unreadable_catalog_is_an_error() {
        assert!(PriceCatalog::parse(b"<html>").is_err());
        assert!(PriceCatalog::parse(b"{}").is_err());
    }
}
