//! Reads of the calling bot's own task state, for the `sylvode://tasks/{id}` and
//! `sylvode://agents/{id}/inbox` resources and for the authorisation of their subscriptions.

use super::{OpenPrClient, urlencoding};
use serde_json::Value;
use uuid::Uuid;

impl OpenPrClient {
    /// `GET /api/v1/ai/tasks/{task_id}` without an attempt: the view of the task's current attempt,
    /// whatever its status. Another bot's task is answered as not found. The caller has already
    /// checked that `task_id` is a canonical UUID.
    pub async fn get_ai_task_current(&self, task_id: &str) -> Result<Value, String> {
        self.get(&format!("/api/v1/ai/tasks/{}", urlencoding::encode(task_id)))
            .await
    }

    /// `GET /api/v1/ai/tasks/active`: the calling bot's open tasks.
    pub async fn list_active_ai_tasks(&self) -> Result<Value, String> {
        self.get("/api/v1/ai/tasks/active").await
    }

    /// The id of the bot this credential belongs to (`GET /api/v1/auth/me`). A credential that
    /// belongs to no bot, or an answer without a valid id, is an error.
    pub async fn bot_identity(&self) -> Result<Uuid, String> {
        let me: Value = self.get("/api/v1/auth/me").await?;
        bot_id_of_me(&me)
    }
}

fn bot_id_of_me(me: &Value) -> Result<Uuid, String> {
    let data = me.get("data").unwrap_or(me);
    data.get("bot")
        .filter(|bot| !bot.is_null())
        .and_then(|bot| bot.get("bot_id"))
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| "Malformed response from /api/v1/auth/me: no bot id".to_string())
}
