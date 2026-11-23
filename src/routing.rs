use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{Config, ProviderConfig, ProviderKind};

mod boss;
pub use boss::ModelRouter;

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ModelTarget {
  #[serde(default, skip_serializing_if = "Option::is_none")]
  pub provider: Option<String>,
  pub model: String,
}

impl ModelTarget {
  pub fn configured(config: &Config) -> Self {
    Self {
      provider: config.provider.clone(),
      model: config.model.clone(),
    }
  }

  pub fn profile(&self, config: &Config) -> Result<ProviderConfig> {
    match &self.provider {
      Some(name) => config
        .providers
        .get(name)
        .cloned()
        .with_context(|| format!("provider {name} is not configured")),
      None => Ok(ProviderConfig::http(&config.endpoint, &config.api_key_env)),
    }
  }

  pub fn provider_name(&self) -> &str {
    self.provider.as_deref().unwrap_or("default")
  }
}

impl std::fmt::Display for ModelTarget {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{} · {}", self.provider_name(), self.model)
  }
}

/// Saved pairs are shared by manual switching, delegation, and boss validation.
pub fn models(config: &Config) -> BTreeMap<ModelTarget, String> {
  let mut models = BTreeMap::new();
  for (name, profile) in &config.providers {
    for model in profile.models.iter().chain(profile.model.iter()) {
      if !model.trim().is_empty() {
        models.insert(
          ModelTarget {
            provider: Some(name.clone()),
            model: model.clone(),
          },
          profile.model_hints.get(model).cloned().unwrap_or_default(),
        );
      }
    }
  }
  if !config.model.trim().is_empty() {
    let target = ModelTarget::configured(config);
    let hint = config
      .active_provider()
      .ok()
      .and_then(|profile| profile.model_hints.get(&target.model).cloned())
      .unwrap_or_default();
    models.insert(target, hint);
  }
  models
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ModelMode {
  Fixed { target: ModelTarget },
  Auto { strategy: RoutingStrategy },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
  Economy,
  #[default]
  Balanced,
  Quality,
}

impl RoutingStrategy {
  pub const ALL: [Self; 3] = [Self::Economy, Self::Balanced, Self::Quality];

  pub fn label(self) -> &'static str {
    match self {
      Self::Economy => "Economy",
      Self::Balanced => "Balanced",
      Self::Quality => "Quality",
    }
  }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCost {
  Low,
  Medium,
  High,
  #[default]
  Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelQuality {
  Basic,
  #[default]
  Capable,
  Strong,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BossCandidate {
  #[serde(flatten)]
  pub target: ModelTarget,
  #[serde(default)]
  pub cost: ModelCost,
  #[serde(default)]
  pub quality: ModelQuality,
  #[serde(default)]
  pub images: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct BossConfig {
  pub enabled: bool,
  pub strategy: RoutingStrategy,
  #[serde(skip_serializing_if = "Option::is_none")]
  pub router: Option<ModelTarget>,
  #[serde(skip_serializing_if = "Vec::is_empty")]
  pub candidates: Vec<BossCandidate>,
}

impl BossConfig {
  pub fn is_default(&self) -> bool {
    self == &Self::default()
  }

  pub fn router_target(&self, config: &Config) -> ModelTarget {
    self
      .router
      .clone()
      .unwrap_or_else(|| ModelTarget::configured(config))
  }

  pub fn validate(&self, config: &Config) -> Result<()> {
    if !self.enabled {
      return Ok(());
    }
    let saved = models(config);
    let mut seen = std::collections::BTreeSet::new();
    ensure!(
      !self.candidates.is_empty(),
      "boss needs at least one candidate"
    );
    for candidate in &self.candidates {
      let target = &candidate.target;
      ensure!(
        saved.contains_key(target),
        "boss candidate {target} is not a saved model"
      );
      ensure!(
        seen.insert(target),
        "boss candidate {target} is listed twice"
      );
      target.profile(config)?.validate(target.provider_name())?;
    }
    if self.candidates.len() > 1 || self.router.is_some() {
      let target = self.router_target(config);
      ensure!(!target.model.trim().is_empty(), "boss router needs a model");
      let profile = target.profile(config)?;
      ensure!(
        profile.kind == ProviderKind::Http,
        "boss router {target} needs an HTTP profile"
      );
      profile.validate(target.provider_name())?;
    }
    Ok(())
  }
}

pub fn model_mode(config: &Config, session: &crate::Session) -> ModelMode {
  if config.pin_model && !session.model_changed {
    return ModelMode::Fixed {
      target: ModelTarget::configured(config),
    };
  }
  if let Some(mode) = &session.model_mode
    && (!matches!(mode, ModelMode::Auto { .. }) || config.boss.enabled)
  {
    return mode.clone();
  }
  if config.boss.enabled {
    return ModelMode::Auto {
      strategy: config.boss.strategy,
    };
  }
  ModelMode::Fixed {
    target: ModelTarget::configured(config),
  }
}
