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

    /// The id of the bot this credential belongs to (`GET /api/v1/auth/me`); `None` for a credential
    /// that belongs to no bot (a user token). An answer that is not an identity at all is an error.
    pub async fn bot_identity(&self) -> Result<Option<Uuid>, String> {
        let me: Value = self.get("/api/v1/auth/me").await?;
        bot_id_of_me(&me)
    }
}

fn bot_id_of_me(me: &Value) -> Result<Option<Uuid>, String> {
    let data = me.get("data").unwrap_or(me);
    if !data.is_object() {
        return Err("Malformed response from /api/v1/auth/me".to_string());
    }
    let Some(bot) = data.get("bot").filter(|bot| !bot.is_null()) else {
        return Ok(None);
    };
    bot.get("bot_id")
        .and_then(Value::as_str)
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .map(Some)
        .ok_or_else(|| "Malformed response from /api/v1/auth/me: no bot id".to_string())
}
