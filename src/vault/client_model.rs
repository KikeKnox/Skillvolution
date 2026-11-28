//! The models each client's tiers map to on this machine (`client_models`): several
//! ranked rows per client and tier, of which file generation uses the first enabled one.

use super::{EFFORTS, TIERS, Vault, validate_single_line};
use crate::clients::ClientKind;
use anyhow::{Result, bail, ensure};
use rusqlite::params;
use serde::Serialize;

/// One stored `client_models` row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ClientModel {
    pub client: String,
    pub provider: String,
    /// Exactly what is written to the file or passed per call.
    pub model: String,
    pub tier: String,
    pub effort: Option<String>,
    /// Lower is preferred; ties break by insertion order.
    pub priority: i64,
    pub enabled: bool,
}

impl ClientModel {
    /// The provider, or `None` when unknown (stored as `''`: a bare model given to `set`,
    /// or data from before v9).
    pub fn provider(&self) -> Option<&str> {
        Some(self.provider.as_str()).filter(|p| !p.is_empty())
    }
}

/// Which rows of a client and tier `remove`, `enable` and `disable` act on.
pub struct ModelSelector<'a> {
    pub client: &'a str,
    pub tier: &'a str,
    pub model: &'a str,
    pub provider: Option<&'a str>,
    pub effort: Option<&'a str>,
}

fn check_client_tier(client: &str, tier: &str) -> Result<()> {
    ensure!(
        ClientKind::with_agents().any(|kind| kind.token() == client),
        "client must be one of {}",
        ClientKind::with_agents()
            .map(ClientKind::token)
            .collect::<Vec<_>>()
            .join(", ")
    );
    ensure!(
        TIERS.contains(&tier),
        "tier must be one of {}",
        TIERS.join(", ")
    );
    Ok(())
}

fn check_effort(effort: Option<&str>) -> Result<()> {
    if let Some(effort) = effort {
        ensure!(
            EFFORTS.contains(&effort),
            "effort must be one of {}",
            EFFORTS.join(", ")
        );
    }
    Ok(())
}

fn check_model(model: &str) -> Result<()> {
    validate_single_line("model", model, 128)?;
    ensure!(
        !model.contains(char::is_whitespace),
        "model must not contain whitespace"
    );
    Ok(())
}

/// The provider a row gets: the explicit one, else `anthropic` for Claude Code or the
/// `provider/` prefix of an OpenCode model. `None` when OpenCode has neither.
fn provider_for(client: &str, model: &str, explicit: Option<&str>) -> Result<Option<String>> {
    if let Some(provider) = explicit {
        validate_single_line("provider", provider, 64)?;
        ensure!(
            !provider.contains(char::is_whitespace),
            "provider must not contain whitespace"
        );
    }
    if client != "opencode" {
        return Ok(Some(explicit.unwrap_or("anthropic").to_owned()));
    }
    let prefix = model
        .split_once('/')
        .map(|(p, _)| p)
        .filter(|p| !p.is_empty());
    if let (Some(given), Some(prefix)) = (explicit, prefix) {
        ensure!(
            given == prefix,
            "provider `{given}` does not match the model's `{prefix}/` prefix"
        );
    }
    Ok(prefix.or(explicit).map(str::to_owned))
}

/// The provider `model` implies for `client` with none given: `anthropic` for Claude
/// Code, the `provider/` prefix for OpenCode, `None` when OpenCode's model has none.
pub fn implied_provider(client: &str, model: &str) -> Option<String> {
    provider_for(client, model, None).ok().flatten()
}

impl Vault {
    /// Every stored row, ordered by client, tier, then preference (priority, insertion).
    pub fn client_models(&self) -> Result<Vec<ClientModel>> {
        let mut statement = self.conn.prepare(
            "SELECT client, provider, model, tier, effort, priority, enabled FROM client_models
             ORDER BY client, tier, priority, id",
        )?;
        Ok(statement
            .query_map([], |row| {
                let effort: String = row.get(4)?;
                Ok(ClientModel {
                    client: row.get(0)?,
                    provider: row.get(1)?,
                    model: row.get(2)?,
                    tier: row.get(3)?,
                    effort: Some(effort).filter(|e| !e.is_empty()),
                    priority: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    /// Maps `tier` to `model` (and optionally `effort`) for `client` as the priority-0
    /// row, replacing every other priority-0 model of that client and tier. A bare OpenCode
    /// model (no `provider/`) is accepted with an empty provider; the CLI warns about it.
    pub fn set_agent_model(
        &self,
        client: &str,
        tier: &str,
        model: &str,
        effort: Option<&str>,
    ) -> Result<()> {
        check_client_tier(client, tier)?;
        check_model(model)?;
        check_effort(effort)?;
        let provider = provider_for(client, model, None)?.unwrap_or_default();
        let effort = effort.unwrap_or("");
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO client_models (client, provider, model, tier, effort, priority, enabled)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 1)
             ON CONFLICT(client, tier, provider, model, effort)
             DO UPDATE SET priority = 0, enabled = 1",
            params![client, provider, model, tier, effort],
        )?;
        tx.execute(
            "DELETE FROM client_models WHERE client = ?1 AND tier = ?2
             AND priority = 0 AND NOT (provider = ?3 AND model = ?4 AND effort = ?5)",
            params![client, tier, provider, model, effort],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes the priority-0 rows of `tier` for `client`, so it reverts to the next
    /// ranked model, the compiled default or none; `false` when there was none.
    pub fn unset_agent_model(&self, client: &str, tier: &str) -> Result<bool> {
        check_client_tier(client, tier)?;
        let removed = self.conn.execute(
            "DELETE FROM client_models WHERE client = ?1 AND tier = ?2 AND priority = 0",
            params![client, tier],
        )?;
        Ok(removed > 0)
    }

    /// Adds a model row; its priority defaults to after the tier's last. Returns the
    /// stored row. Errors when the same row exists.
    pub fn add_client_model(
        &self,
        client: &str,
        tier: &str,
        model: &str,
        provider: Option<&str>,
        effort: Option<&str>,
        priority: Option<i64>,
    ) -> Result<ClientModel> {
        check_client_tier(client, tier)?;
        check_model(model)?;
        check_effort(effort)?;
        let Some(provider) = provider_for(client, model, provider)? else {
            bail!(
                "OpenCode models are `provider/model`: `{model}` has no provider (or pass --provider)"
            );
        };
        if let Some(priority) = priority {
            ensure!(priority >= 0, "priority must not be negative");
        }
        let effort = effort.unwrap_or("");
        let tx = self.conn.unchecked_transaction()?;
        let priority: i64 = match priority {
            Some(p) => p,
            None => tx.query_row(
                "SELECT COALESCE(MAX(priority) + 1, 0) FROM client_models WHERE client = ?1 AND tier = ?2",
                params![client, tier],
                |row| row.get(0),
            )?,
        };
        let inserted = tx.execute(
            "INSERT INTO client_models (client, provider, model, tier, effort, priority)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
            params![client, provider, model, tier, effort, priority],
        )?;
        ensure!(
            inserted > 0,
            "{client} {tier} already has {provider} `{model}`{}; remove it first or use enable",
            if effort.is_empty() {
                String::new()
            } else {
                format!(" with effort {effort}")
            }
        );
        tx.commit()?;
        Ok(ClientModel {
            client: client.into(),
            provider,
            model: model.into(),
            tier: tier.into(),
            effort: Some(effort.to_owned()).filter(|e| !e.is_empty()),
            priority,
            enabled: true,
        })
    }

    /// Deletes the rows `selector` names; the number removed.
    pub fn remove_client_model(&self, selector: &ModelSelector) -> Result<usize> {
        self.change_client_models(selector, "DELETE FROM client_models")
    }

    /// Enables or disables the rows `selector` names; the number matched.
    pub fn set_client_model_enabled(
        &self,
        selector: &ModelSelector,
        enabled: bool,
    ) -> Result<usize> {
        self.change_client_models(
            selector,
            if enabled {
                "UPDATE client_models SET enabled = 1"
            } else {
                "UPDATE client_models SET enabled = 0"
            },
        )
    }

    fn change_client_models(&self, selector: &ModelSelector, verb: &str) -> Result<usize> {
        check_client_tier(selector.client, selector.tier)?;
        check_effort(selector.effort)?;
        Ok(self.conn.execute(
            &format!(
                "{verb} WHERE client = ?1 AND tier = ?2 AND model = ?3
                 AND (?4 IS NULL OR provider = ?4) AND (?5 IS NULL OR effort = ?5)"
            ),
            params![
                selector.client,
                selector.tier,
                selector.model,
                selector.provider,
                selector.effort
            ],
        )?)
    }
}
