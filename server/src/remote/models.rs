//! Lists the models a phone can choose from, and what of each it can set.
use serde::Serialize;
use serde_json::{json, Value};

use crate::{
    cursor::{
        protocol::proto::agent::v1 as pb,
        services::model_catalog::{model_choices, plugin_model_choices, ModelChoices},
    },
    model::{format_token_count, parse_token_count, ModelConfig, ModelType},
    plugin::PluginRegistry,
    store::Store,
    Result,
};

/// What Cursor calls the two, the name it sends first.
const CONTEXT: [&str; 1] = ["context"];
const EFFORT: [&str; 2] = ["reasoning", "effort"];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(super) struct Model {
    pub id: String,
    pub label: String,
    /// As its maker names it; the label may be anything.
    pub name: String,
    #[serde(skip)]
    pub context: Setting,
    #[serde(skip)]
    pub effort: Setting,
}

/// One thing that can be set, and what it is while nobody sets it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Setting {
    pub choices: Vec<(String, String)>,
    pub usual: Option<String>,
}

impl Setting {
    pub fn has(&self, id: &str) -> bool {
        self.choices.iter().any(|(choice, _)| choice == id)
    }

    /// `None` where there is nothing to choose.
    fn view(&self, set: Option<&str>, label: impl Fn(&str) -> String) -> Value {
        if self.choices.is_empty() {
            return Value::Null;
        }
        let now = set.or(self.usual.as_deref());
        let name = |id: &str| {
            let known = self.choices.iter().find(|(choice, _)| choice == id);
            known.map_or_else(|| label(id), |(_, name)| name.clone())
        };
        json!({
            "now": now,
            "label": now.map(name),
            "items": self.choices.iter().map(|(id, name)| json!({"id": id, "label": name})).collect::<Vec<_>>(),
        })
    }
}

impl Model {
    /// A model known by its name only, as one no longer configured is.
    pub fn named(id: String, label: String, name: String) -> Self {
        Self {
            id,
            label,
            name,
            context: Setting::default(),
            effort: Setting::default(),
        }
    }

    fn new(id: String, label: String, name: String, choices: ModelChoices) -> Self {
        let mut model = Self::named(id, label, name);
        model.context.choices = choices.contexts;
        model.effort.choices = choices.efforts;
        model
    }

    fn configured(model: ModelConfig) -> Self {
        let choices = model_choices(&model);
        let usual_context = model.context_window_tokens.map(|tokens| {
            let same = |(id, _): &&(String, String)| parse_token_count(id) == Some(tokens);
            let choice = choices.contexts.iter().find(same);
            choice.map_or_else(|| tokens.to_string(), |(id, _)| id.clone())
        });
        let usual_effort = match model.model_type {
            ModelType::OpenAi => model.reasoning_effort,
            ModelType::Anthropic => model.anthropic_thinking_effort,
        };
        let mut known = Self::new(
            model.model_hash,
            model.display_name,
            model.model_id,
            choices,
        );
        known.context.usual = usual_context;
        known.effort.usual = usual_effort;
        known
    }

    /// What the phone shows of the two, for a request as it would be sent.
    pub fn tuning(&self, request: &pb::RequestedModel) -> Value {
        let context = parameter(request, &CONTEXT);
        let effort = parameter(request, &EFFORT).filter(|effort| !effort.is_empty());
        json!({
            "context": self.context.view(context, |id| {
                parse_token_count(id).map_or_else(|| id.to_owned(), format_token_count)
            }),
            "effort": self.effort.view(effort, str::to_owned),
        })
    }
}

pub(super) fn set_context(request: &mut pb::RequestedModel, value: &str) {
    set(request, &CONTEXT, value);
}

pub(super) fn set_effort(request: &mut pb::RequestedModel, value: &str) {
    set(request, &EFFORT, value);
}

fn parameter<'a>(request: &'a pb::RequestedModel, names: &[&str]) -> Option<&'a str> {
    let mut parameters = request.parameters.iter();
    parameters
        .find(|it| names.contains(&it.id.as_str()))
        .map(|it| it.value.as_str())
}

fn set(request: &mut pb::RequestedModel, names: &[&str], value: &str) {
    let mut known = request.parameters.iter_mut();
    match known.find(|it| names.contains(&it.id.as_str())) {
        Some(parameter) => parameter.value = value.into(),
        None => request
            .parameters
            .push(pb::requested_model::ModelParameterValue {
                id: names[0].into(),
                value: value.into(),
            }),
    }
}

#[derive(Clone)]
pub(super) struct Models {
    store: Store,
    plugins: Option<PluginRegistry>,
}

impl Models {
    pub fn new(store: Store, plugins: Option<PluginRegistry>) -> Self {
        Self { store, plugins }
    }

    pub async fn list(&self) -> Result<Vec<Model>> {
        let configured = self.store.configured_models().await?;
        let plugins = match &self.plugins {
            Some(plugins) => plugins.configured_models().await,
            None => Vec::new(),
        };
        let configured = configured.into_iter().map(Model::configured);
        let plugins = plugins.into_iter().map(|model| {
            let name = model.id.clone();
            Model::new(model.id, model.display_name, name, plugin_model_choices())
        });
        Ok(configured.chain(plugins).collect())
    }

    /// The model Cursor asks for by this id.
    pub async fn find(&self, id: &str) -> Result<Option<Model>> {
        let models = self.list().await?;
        Ok(models.into_iter().find(|model| model.id == id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> Model {
        let mut model = Model::new(
            "id".into(),
            "Mine".into(),
            "maker-1".into(),
            plugin_model_choices(),
        );
        model.effort.usual = Some("minimal".into());
        model
    }

    #[test]
    fn what_is_set_shows_by_name_and_what_is_not_shows_as_configured() {
        let mut request = pb::RequestedModel::default();
        let usual = model().tuning(&request);
        assert_eq!(usual["context"]["now"], Value::Null);
        assert_eq!(usual["effort"]["label"], "minimal");
        set_context(&mut request, "1m");
        set_effort(&mut request, "xhigh");
        let set = model().tuning(&request);
        assert_eq!(set["context"]["label"], "1M");
        assert_eq!(set["effort"]["label"], "Extra High");
    }

    #[test]
    fn setting_again_replaces_what_was_set_by_whichever_name() {
        let mut request = pb::RequestedModel::default();
        request
            .parameters
            .push(pb::requested_model::ModelParameterValue {
                id: "effort".into(),
                value: "low".into(),
            });
        set_effort(&mut request, "max");
        assert_eq!(request.parameters.len(), 1);
        assert_eq!(model().tuning(&request)["effort"]["now"], "max");
    }

    #[test]
    fn a_model_known_only_by_name_has_nothing_to_set() {
        let model = Model::named("id".into(), "Gone".into(), "gone".into());
        let tuning = model.tuning(&pb::RequestedModel::default());
        assert_eq!(tuning, json!({"context": null, "effort": null}));
    }
}
