use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{FromRow, SqlitePool};

/// Sidebar entry for a stored Playground conversation.
#[derive(Debug, Clone, Serialize, FromRow)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub protocol: String,
    pub model: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i64,
}

/// One message as returned to the admin UI. `tools` and `meta` stay as JSON so
/// the UI can keep gateway diagnostics without a second schema.
#[derive(Debug, Clone, Serialize)]
pub struct StoredMessage {
    pub role: String,
    pub content: String,
    pub reasoning: String,
    pub tools: Value,
    pub status: String,
    pub error: String,
    pub meta: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct StoredSession {
    pub id: String,
    pub title: String,
    pub protocol: String,
    pub model: String,
    pub system: String,
    pub stream: bool,
    pub max_tokens: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<StoredMessage>,
}

/// Full replacement payload. The UI saves the whole conversation after each
/// turn, so a single transactional upsert keeps it consistent.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionInput {
    #[serde(default)]
    pub title: String,
    pub protocol: String,
    pub model: String,
    #[serde(default)]
    pub system: String,
    #[serde(default = "default_stream")]
    pub stream: bool,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i64,
    #[serde(default)]
    pub messages: Vec<MessageInput>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageInput {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub reasoning: String,
    #[serde(default)]
    pub tools: Value,
    #[serde(default = "default_message_status")]
    pub status: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub meta: Value,
}

fn default_stream() -> bool {
    true
}

fn default_max_tokens() -> i64 {
    4096
}

fn default_message_status() -> String {
    "done".to_string()
}

#[derive(Debug, FromRow)]
struct SessionHeaderRow {
    id: String,
    title: String,
    protocol: String,
    model: String,
    system: String,
    stream: bool,
    max_tokens: i64,
    created_at: i64,
    updated_at: i64,
}

#[derive(Debug, FromRow)]
struct MessageRow {
    role: String,
    content: String,
    reasoning: String,
    tools_json: String,
    status: String,
    error: String,
    meta_json: String,
}

const LIST_LIMIT: i64 = 200;
const MAX_TITLE_CHARS: usize = 200;
const MAX_MODEL_CHARS: usize = 256;
const MAX_SYSTEM_BYTES: usize = 64 * 1024;
const MAX_MESSAGE_BYTES: usize = 512 * 1024;
const MAX_ERROR_BYTES: usize = 4 * 1024;
const MAX_JSON_BYTES: usize = 64 * 1024;
const MAX_MESSAGES: usize = 1000;
const MAX_SESSION_BYTES: usize = 1024 * 1024;
const MAX_MAX_TOKENS: i64 = 1_000_000;

/// Validate a full replacement payload before it reaches SQLite.
///
/// Bounds protect the single-writer database from a runaway client while
/// staying far above realistic Playground conversations.
pub fn validate_input(input: &SessionInput) -> Result<(), String> {
    if !matches!(input.protocol.as_str(), "chat" | "responses" | "anthropic") {
        return Err("protocol must be chat, responses or anthropic".to_string());
    }
    if input.title.trim().chars().count() > MAX_TITLE_CHARS {
        return Err(format!(
            "title must be at most {MAX_TITLE_CHARS} characters"
        ));
    }
    if input.model.trim().is_empty() {
        return Err("model is required".to_string());
    }
    if input.model.chars().count() > MAX_MODEL_CHARS {
        return Err(format!(
            "model must be at most {MAX_MODEL_CHARS} characters"
        ));
    }
    if input.system.len() > MAX_SYSTEM_BYTES {
        return Err(format!(
            "system prompt must be at most {} bytes",
            MAX_SYSTEM_BYTES
        ));
    }
    if !(1..=MAX_MAX_TOKENS).contains(&input.max_tokens) {
        return Err(format!("max_tokens must be between 1 and {MAX_MAX_TOKENS}"));
    }
    if input.messages.len() > MAX_MESSAGES {
        return Err(format!(
            "a session may contain at most {MAX_MESSAGES} messages"
        ));
    }

    let mut total_bytes = input.system.len() + input.model.len() + input.title.len();
    for message in &input.messages {
        if !matches!(message.role.as_str(), "user" | "assistant") {
            return Err("message role must be user or assistant".to_string());
        }
        if !matches!(
            message.status.as_str(),
            "done" | "streaming" | "error" | "stopped"
        ) {
            return Err("message status must be done, streaming, error or stopped".to_string());
        }
        if message.content.len() > MAX_MESSAGE_BYTES {
            return Err(format!(
                "message content must be at most {} bytes",
                MAX_MESSAGE_BYTES
            ));
        }
        if message.reasoning.len() > MAX_MESSAGE_BYTES {
            return Err(format!(
                "message reasoning must be at most {} bytes",
                MAX_MESSAGE_BYTES
            ));
        }
        if message.error.len() > MAX_ERROR_BYTES {
            return Err(format!(
                "message error must be at most {} bytes",
                MAX_ERROR_BYTES
            ));
        }
        let tools_bytes = json_bytes(&message.tools);
        let meta_bytes = json_bytes(&message.meta);
        if tools_bytes > MAX_JSON_BYTES || meta_bytes > MAX_JSON_BYTES {
            return Err(format!(
                "message tools/meta must be at most {} bytes each",
                MAX_JSON_BYTES
            ));
        }
        total_bytes += message.content.len()
            + message.reasoning.len()
            + message.error.len()
            + tools_bytes
            + meta_bytes;
    }
    if total_bytes > MAX_SESSION_BYTES {
        return Err(format!(
            "session must be at most {} bytes",
            MAX_SESSION_BYTES
        ));
    }

    Ok(())
}

fn json_bytes(value: &Value) -> usize {
    if value.is_null() {
        0
    } else {
        value.to_string().len()
    }
}

fn normalized_title(input: &SessionInput) -> String {
    let title: String = input.title.trim().chars().take(MAX_TITLE_CHARS).collect();
    if !title.is_empty() {
        return title;
    }
    input
        .messages
        .iter()
        .find(|message| message.role == "user" && !message.content.trim().is_empty())
        .map(|message| message.content.trim().chars().take(40).collect())
        .unwrap_or_default()
}

fn tools_json(value: &Value) -> String {
    if value.is_array() {
        value.to_string()
    } else {
        "[]".to_string()
    }
}

fn meta_json(value: &Value) -> String {
    if value.is_object() {
        value.to_string()
    } else {
        "{}".to_string()
    }
}

fn parse_tools(raw: &str) -> Value {
    serde_json::from_str(raw)
        .ok()
        .filter(|value: &Value| value.is_array())
        .unwrap_or_else(|| Value::Array(Vec::new()))
}

fn parse_meta(raw: &str) -> Value {
    serde_json::from_str(raw)
        .ok()
        .filter(|value: &Value| value.is_object())
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()))
}

pub async fn list(pool: &SqlitePool) -> sqlx::Result<Vec<SessionSummary>> {
    sqlx::query_as(
        "SELECT s.id, s.title, s.protocol, s.model, s.created_at, s.updated_at,
                (SELECT COUNT(*) FROM playground_messages m WHERE m.session_id = s.id)
                    AS message_count
         FROM playground_sessions s
         ORDER BY s.updated_at DESC
         LIMIT ?",
    )
    .bind(LIST_LIMIT)
    .fetch_all(pool)
    .await
}

pub async fn get(pool: &SqlitePool, id: &str) -> sqlx::Result<Option<StoredSession>> {
    let Some(header) = sqlx::query_as::<_, SessionHeaderRow>(
        "SELECT id, title, protocol, model, system, stream, max_tokens, created_at, updated_at
         FROM playground_sessions WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let rows = sqlx::query_as::<_, MessageRow>(
        "SELECT role, content, reasoning, tools_json, status, error, meta_json
         FROM playground_messages WHERE session_id = ? ORDER BY id ASC",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let messages = rows
        .into_iter()
        .map(|row| StoredMessage {
            role: row.role,
            content: row.content,
            reasoning: row.reasoning,
            tools: parse_tools(&row.tools_json),
            status: row.status,
            error: row.error,
            meta: parse_meta(&row.meta_json),
        })
        .collect();

    Ok(Some(StoredSession {
        id: header.id,
        title: header.title,
        protocol: header.protocol,
        model: header.model,
        system: header.system,
        stream: header.stream,
        max_tokens: header.max_tokens,
        created_at: header.created_at,
        updated_at: header.updated_at,
        messages,
    }))
}

pub async fn save(pool: &SqlitePool, id: &str, input: &SessionInput, now: i64) -> sqlx::Result<()> {
    let title = normalized_title(input);
    let mut tx = pool.begin().await?;

    // Keep the original created_at on updates; only the conversation content
    // and updated_at move forward.
    sqlx::query(
        "INSERT INTO playground_sessions
         (id, title, protocol, model, system, stream, max_tokens, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
             title = excluded.title,
             protocol = excluded.protocol,
             model = excluded.model,
             system = excluded.system,
             stream = excluded.stream,
             max_tokens = excluded.max_tokens,
             updated_at = excluded.updated_at",
    )
    .bind(id)
    .bind(&title)
    .bind(&input.protocol)
    .bind(input.model.trim())
    .bind(&input.system)
    .bind(input.stream)
    .bind(input.max_tokens)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM playground_messages WHERE session_id = ?")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    for message in &input.messages {
        sqlx::query(
            "INSERT INTO playground_messages
             (session_id, role, content, reasoning, tools_json, status, error, meta_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&message.role)
        .bind(&message.content)
        .bind(&message.reasoning)
        .bind(tools_json(&message.tools))
        .bind(&message.status)
        .bind(&message.error)
        .bind(meta_json(&message.meta))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await
}

pub async fn delete(pool: &SqlitePool, id: &str) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM playground_sessions WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::test_support;

    fn sample_input() -> SessionInput {
        SessionInput {
            title: String::new(),
            protocol: "chat".to_string(),
            model: "deepseek-reasoner".to_string(),
            system: "你是助手".to_string(),
            stream: true,
            max_tokens: 4096,
            messages: vec![
                MessageInput {
                    role: "user".to_string(),
                    content: "介绍一下 OneAPI".to_string(),
                    reasoning: String::new(),
                    tools: Value::Null,
                    status: "done".to_string(),
                    error: String::new(),
                    meta: Value::Null,
                },
                MessageInput {
                    role: "assistant".to_string(),
                    content: "OneAPI 是一个网关。".to_string(),
                    reasoning: "先解释定位，再说明协议。".to_string(),
                    tools: json!([]),
                    status: "done".to_string(),
                    error: String::new(),
                    meta: json!({"channel": "mock", "elapsed_ms": 1200}),
                },
            ],
        }
    }

    #[tokio::test]
    async fn round_trip_preserves_messages_and_reasoning() {
        let pool = test_support::pool().await;
        let id = uuid::Uuid::new_v4().to_string();
        let input = sample_input();
        validate_input(&input).expect("valid input");

        save(&pool, &id, &input, 1_000).await.expect("save");
        let stored = get(&pool, &id).await.expect("get").expect("exists");
        assert_eq!(stored.title, "介绍一下 OneAPI");
        assert_eq!(stored.messages.len(), 2);
        assert_eq!(stored.messages[1].reasoning, "先解释定位，再说明协议。");
        assert_eq!(stored.messages[1].meta["channel"], "mock");

        let items = list(&pool).await.expect("list");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].message_count, 2);

        assert!(delete(&pool, &id).await.expect("delete"));
        assert!(get(&pool, &id).await.expect("get after delete").is_none());
        assert!(list(&pool).await.expect("list after delete").is_empty());
    }

    #[tokio::test]
    async fn saving_again_replaces_messages_and_keeps_created_at() {
        let pool = test_support::pool().await;
        let id = uuid::Uuid::new_v4().to_string();
        let mut input = sample_input();

        save(&pool, &id, &input, 1_000).await.expect("first save");
        input.messages.pop();
        input.title = "更新后的标题".to_string();
        save(&pool, &id, &input, 2_000).await.expect("second save");

        let stored = get(&pool, &id).await.expect("get").expect("exists");
        assert_eq!(stored.created_at, 1_000);
        assert_eq!(stored.updated_at, 2_000);
        assert_eq!(stored.messages.len(), 1);
        assert_eq!(stored.title, "更新后的标题");
    }

    #[test]
    fn rejects_oversized_or_invalid_input() {
        let pool_input = sample_input();
        assert!(validate_input(&pool_input).is_ok());

        let mut invalid = sample_input();
        invalid.protocol = "grpc".to_string();
        assert!(validate_input(&invalid).is_err());

        let mut invalid = sample_input();
        invalid.messages[0].role = "system".to_string();
        assert!(validate_input(&invalid).is_err());

        let mut invalid = sample_input();
        invalid.system = "x".repeat(MAX_SYSTEM_BYTES + 1);
        assert!(validate_input(&invalid).is_err());
    }
}
