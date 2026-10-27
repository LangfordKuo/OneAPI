use sqlx::{FromRow, SqlitePool};

#[allow(dead_code)]
#[derive(Debug, Clone, FromRow)]
pub struct StoredResponse {
    pub id: String,
    pub api_key_id: i64,
    pub model: String,
    pub request_json: String,
    pub response_json: String,
    pub status: String,
    pub created_at: i64,
}

#[allow(clippy::too_many_arguments)]
pub async fn insert(
    pool: &SqlitePool,
    id: &str,
    api_key_id: i64,
    model: &str,
    request_json: &str,
    response_json: &str,
    status: &str,
    created_at: i64,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO responses
         (id, api_key_id, model, request_json, response_json, status, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(id) DO UPDATE SET
            api_key_id = excluded.api_key_id,
            model = excluded.model,
            request_json = excluded.request_json,
            response_json = excluded.response_json,
            status = excluded.status,
            created_at = excluded.created_at",
    )
    .bind(id)
    .bind(api_key_id)
    .bind(model)
    .bind(request_json)
    .bind(response_json)
    .bind(status)
    .bind(created_at)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get(
    pool: &SqlitePool,
    id: &str,
    api_key_id: i64,
) -> sqlx::Result<Option<StoredResponse>> {
    sqlx::query_as(
        "SELECT id, api_key_id, model, request_json, response_json, status, created_at
         FROM responses WHERE id = ? AND api_key_id = ?",
    )
    .bind(id)
    .bind(api_key_id)
    .fetch_optional(pool)
    .await
}

pub async fn delete(pool: &SqlitePool, id: &str, api_key_id: i64) -> sqlx::Result<bool> {
    let result = sqlx::query("DELETE FROM responses WHERE id = ? AND api_key_id = ?")
        .bind(id)
        .bind(api_key_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
