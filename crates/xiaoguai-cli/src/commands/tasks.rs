//! `xiaoguai tasks` subcommand — kanban task board CLI parity
//!
//! Backend ships in v1.4; until then commands return informative errors
//! when /v1/tasks endpoints respond 404 or 503.
//!
//! # CLI surface
//!
//! ```text
//! xiaoguai tasks list --board default --column triage
//! xiaoguai tasks create --title "Fix auth bug" --board default
//! xiaoguai tasks move <task-id> --to running
//! xiaoguai tasks claim <task-id>
//! xiaoguai tasks complete <task-id> --outcome "deployed to prod"
//! xiaoguai tasks block <task-id> --reason "waiting on infra"
//! xiaoguai tasks dispatch --board default --n 2
//! xiaoguai tasks show <task-id>
//! ```

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

const NOT_YET_AVAILABLE: &str =
    "Tasks subsystem not yet wired (ships in v1.4). See ADR-0019.";

/// Thin HTTP client scoped to the `/v1/tasks` namespace.
pub struct TasksClient {
    base_url: String,
    http: reqwest::Client,
}

impl TasksClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }

    /// Map a reqwest `Response` to `Ok(JsonValue)` or a user-friendly error.
    async fn handle_response(&self, resp: reqwest::Response) -> Result<JsonValue> {
        let status = resp.status();
        if status.as_u16() == 404 || status.as_u16() == 503 {
            return Err(anyhow!("{NOT_YET_AVAILABLE}"));
        }
        if status.as_u16() == 422 {
            let body: JsonValue = resp
                .json()
                .await
                .unwrap_or(serde_json::json!({"detail": "validation error"}));
            let detail = body
                .get("detail")
                .and_then(JsonValue::as_str)
                .unwrap_or("validation error");
            return Err(anyhow!("validation error: {detail}"));
        }
        if !status.is_success() {
            return Err(anyhow!("server returned HTTP {status}"));
        }
        let body: JsonValue = resp.json().await.context("decode response body")?;
        Ok(body)
    }

    /// `GET /v1/tasks?board=…&column=…`
    pub async fn list(&self, board: &str, column: Option<&str>) -> Result<JsonValue> {
        let mut url = format!("{}/v1/tasks?board={board}", self.base_url);
        if let Some(col) = column {
            url.push_str(&format!("&column={col}"));
        }
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .context("GET /v1/tasks")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks`
    pub async fn create(&self, req: &CreateTaskRequest) -> Result<JsonValue> {
        let resp = self
            .http
            .post(format!("{}/v1/tasks", self.base_url))
            .json(req)
            .send()
            .await
            .context("POST /v1/tasks")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks/:id/move`
    pub async fn move_task(&self, task_id: &str, to: &str) -> Result<JsonValue> {
        let resp = self
            .http
            .post(format!("{}/v1/tasks/{task_id}/move", self.base_url))
            .json(&serde_json::json!({ "column": to }))
            .send()
            .await
            .context("POST /v1/tasks/:id/move")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks/:id/claim`
    pub async fn claim(&self, task_id: &str, agent: Option<&str>) -> Result<JsonValue> {
        let mut body = serde_json::json!({});
        if let Some(a) = agent {
            body["agent"] = JsonValue::String(a.to_owned());
        }
        let resp = self
            .http
            .post(format!("{}/v1/tasks/{task_id}/claim", self.base_url))
            .json(&body)
            .send()
            .await
            .context("POST /v1/tasks/:id/claim")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks/:id/complete`
    pub async fn complete(&self, task_id: &str, outcome: Option<&str>) -> Result<JsonValue> {
        let mut body = serde_json::json!({});
        if let Some(o) = outcome {
            body["outcome"] = JsonValue::String(o.to_owned());
        }
        let resp = self
            .http
            .post(format!("{}/v1/tasks/{task_id}/complete", self.base_url))
            .json(&body)
            .send()
            .await
            .context("POST /v1/tasks/:id/complete")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks/:id/block`
    pub async fn block(&self, task_id: &str, reason: &str) -> Result<JsonValue> {
        let resp = self
            .http
            .post(format!("{}/v1/tasks/{task_id}/block", self.base_url))
            .json(&serde_json::json!({ "reason": reason }))
            .send()
            .await
            .context("POST /v1/tasks/:id/block")?;
        self.handle_response(resp).await
    }

    /// `POST /v1/tasks/dispatch?board=…&n=…`
    pub async fn dispatch(&self, board: &str, n: usize) -> Result<JsonValue> {
        let resp = self
            .http
            .post(format!(
                "{}/v1/tasks/dispatch?board={board}&n={n}",
                self.base_url
            ))
            .json(&serde_json::json!({}))
            .send()
            .await
            .context("POST /v1/tasks/dispatch")?;
        self.handle_response(resp).await
    }

    /// `GET /v1/tasks/:id`
    pub async fn show(&self, task_id: &str) -> Result<JsonValue> {
        let resp = self
            .http
            .get(format!("{}/v1/tasks/{task_id}", self.base_url))
            .send()
            .await
            .context("GET /v1/tasks/:id")?;
        self.handle_response(resp).await
    }
}

/// Body for task creation.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub board: String,
    pub column: String,
}

/// Pretty-print a JSON value from the server.
pub fn pretty(v: &JsonValue) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_request_skips_none_description() {
        let req = CreateTaskRequest {
            title: "T".into(),
            description: None,
            board: "b".into(),
            column: "triage".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("description"), "None description must be skipped");
    }

    #[test]
    fn create_request_includes_some_description() {
        let req = CreateTaskRequest {
            title: "T".into(),
            description: Some("desc".into()),
            board: "b".into(),
            column: "triage".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("desc"));
    }
}
