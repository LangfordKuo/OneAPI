use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

/// Upper bound for the indexed `model` / `upstream_model` columns. The request
/// path rejects longer names, and the usage writer clamps anything that still
/// reaches it, so an indexed value can never grow unbounded.
pub const MAX_MODEL_NAME_LEN: usize = 256;

#[derive(Debug, Clone, FromRow)]
pub struct UsageLogRow {
    pub id: i64,
    pub request_id: String,
    pub source: String,
    pub api_key_id: Option<i64>,
    pub channel_id: Option<i64>,
    pub model: String,
    pub upstream_model: Option<String>,
    pub inbound_protocol: String,
    pub upstream_protocol: String,
    pub stream: i64,
    pub status_code: i64,
    pub attempts: i64,
    pub error_type: Option<String>,
    pub error_message: Option<String>,
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    pub total_tokens: Option<i64>,
    pub cached_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub reasoning_tokens: Option<i64>,
    pub latency_ms: i64,
    pub first_token_ms: Option<i64>,
    pub created_at: i64,
    pub api_key_name: Option<String>,
    pub channel_name: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct LogFilters {
    pub since: Option<i64>,
    pub model: Option<String>,
    pub channel_id: Option<i64>,
    pub api_key_id: Option<i64>,
    pub status: Option<String>,
    pub inbound_protocol: Option<String>,
    pub source: Option<String>,
    pub stream: Option<bool>,
    pub request_id: Option<String>,
}

const SELECT: &str = "SELECT l.id, l.request_id, l.source, l.api_key_id, l.channel_id,
    l.model, l.upstream_model, l.inbound_protocol, l.upstream_protocol, l.stream,
    l.status_code, l.attempts, l.error_type, l.error_message, l.prompt_tokens,
    l.completion_tokens, l.total_tokens, l.cached_tokens, l.cache_creation_tokens,
    l.reasoning_tokens, l.latency_ms, l.first_token_ms, l.created_at,
    k.name AS api_key_name, c.name AS channel_name
    FROM usage_logs l
    LEFT JOIN api_keys k ON k.id = l.api_key_id
    LEFT JOIN channels c ON c.id = l.channel_id";

fn push_filters(query: &mut QueryBuilder<'_, Sqlite>, filters: &LogFilters) {
    if let Some(since) = filters.since {
        query.push(" AND l.created_at >= ").push_bind(since);
    }
    if let Some(model) = filters
        .model
        .as_deref()
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        query
            .push(" AND l.model GLOB ")
            .push_bind(glob_prefix(model));
    }
    if let Some(channel_id) = filters.channel_id {
        query.push(" AND l.channel_id = ").push_bind(channel_id);
    }
    if let Some(api_key_id) = filters.api_key_id {
        query.push(" AND l.api_key_id = ").push_bind(api_key_id);
    }
    if let Some(status) = filters.status.as_deref() {
        match status {
            "success" => {
                query.push(" AND l.status_code < 400");
            }
            "error" => {
                query.push(" AND l.status_code >= 400");
            }
            _ => {}
        }
    }
    if let Some(protocol) = filters.inbound_protocol.as_deref() {
        query
            .push(" AND l.inbound_protocol = ")
            .push_bind(protocol.to_string());
    }
    if let Some(source) = filters.source.as_deref() {
        query.push(" AND l.source = ").push_bind(source.to_string());
    }
    if let Some(stream) = filters.stream {
        query
            .push(" AND l.stream = ")
            .push_bind(if stream { 1i64 } else { 0i64 });
    }
    if let Some(request_id) = filters
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|request_id| !request_id.is_empty())
    {
        query
            .push(" AND l.request_id GLOB ")
            .push_bind(glob_prefix(request_id));
    }
}

/// Prefix match that keeps SQLite on the BINARY index. GLOB metacharacters in
/// user input are escaped instead of widening the pattern.
fn glob_prefix(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 1);
    for character in value.chars() {
        match character {
            '*' | '?' | '[' | ']' => {
                escaped.push('[');
                escaped.push(character);
                escaped.push(']');
            }
            _ => escaped.push(character),
        }
    }
    escaped.push('*');
    escaped
}

pub async fn list(
    pool: &SqlitePool,
    filters: &LogFilters,
    limit: i64,
    offset: i64,
    include_total: bool,
) -> sqlx::Result<(Vec<UsageLogRow>, Option<i64>)> {
    // The WHERE keeps push_filters' " AND ..." clauses attached to the base
    // table; without it they bind to the last JOIN's ON clause and stop
    // filtering rows.
    let mut query = QueryBuilder::<Sqlite>::new(SELECT);
    query.push(" WHERE 1=1");
    push_filters(&mut query, filters);
    query
        .push(" ORDER BY l.created_at DESC, l.id DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    let rows = query
        .build_query_as::<UsageLogRow>()
        .fetch_all(pool)
        .await?;

    if !include_total {
        return Ok((rows, None));
    }

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM usage_logs l WHERE 1=1");
    push_filters(&mut count, filters);
    let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

    Ok((rows, Some(total)))
}

/// Fetch one usage row by its immutable primary key. The client-supplied
/// `request_id` is not unique, so it must never select a row on its own.
pub async fn find_by_id(pool: &SqlitePool, id: i64) -> sqlx::Result<Option<UsageLogRow>> {
    let query = format!("{SELECT} WHERE l.id = ?");
    sqlx::query_as(&query).bind(id).fetch_optional(pool).await
}

/// Distinct models observed recently. Used to populate model pickers for
/// channels that only declare the `*` wildcard.
pub async fn recent_models(pool: &SqlitePool, since: i64, limit: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT model FROM usage_logs
         WHERE created_at >= ?
         GROUP BY model
         ORDER BY MAX(created_at) DESC
         LIMIT ?",
    )
    .bind(since)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Distinct models observed recently by one API key. The gateway model list
/// must not reveal model names that belong to other keys, so the wildcard
/// catalog is scoped to the caller's own traffic.
pub async fn recent_models_for_key(
    pool: &SqlitePool,
    since: i64,
    limit: i64,
    api_key_id: i64,
) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT model FROM usage_logs
         WHERE created_at >= ? AND api_key_id = ?
         GROUP BY model
         ORDER BY MAX(created_at) DESC
         LIMIT ?",
    )
    .bind(since)
    .bind(api_key_id)
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Fetch the request/response bodies belonging to one usage row. The detail is
/// matched on the exact `(request_id, detail_api_key_id)` pair recorded when
/// the usage row was written; `detail_api_key_id` survives the deletion of the
/// API key, and playground requests map to `0` like `request_details` does.
/// There is deliberately no "newest row for this request_id" fallback: that
/// would leak another key's bodies for a client-chosen request id.
pub async fn detail_bodies(
    pool: &SqlitePool,
    id: i64,
) -> sqlx::Result<Option<(Option<String>, Option<String>)>> {
    sqlx::query_as(
        "SELECT d.request_body, d.response_body
         FROM usage_logs l
         JOIN request_details d
           ON d.request_id = l.request_id
          AND d.api_key_id = COALESCE(l.detail_api_key_id, 0)
         WHERE l.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{insert_key, pool};

    async fn insert_usage(
        pool: &SqlitePool,
        request_id: &str,
        api_key_id: Option<i64>,
        detail_api_key_id: Option<i64>,
        model: &str,
    ) -> i64 {
        let result = sqlx::query(
            "INSERT INTO usage_logs
             (request_id, source, api_key_id, detail_api_key_id, model, inbound_protocol,
              upstream_protocol, status_code, latency_ms, created_at)
             VALUES (?, 'gateway', ?, ?, ?, 'chat', 'chat', 200, 5, 1)",
        )
        .bind(request_id)
        .bind(api_key_id)
        .bind(detail_api_key_id)
        .bind(model)
        .execute(pool)
        .await
        .expect("insert usage log");
        result.last_insert_rowid()
    }

    async fn insert_detail(
        pool: &SqlitePool,
        request_id: &str,
        api_key_id: i64,
        request_body: &str,
        response_body: &str,
    ) {
        sqlx::query(
            "INSERT INTO request_details
             (request_id, api_key_id, request_body, response_body, created_at)
             VALUES (?, ?, ?, ?, 1)",
        )
        .bind(request_id)
        .bind(api_key_id)
        .bind(request_body)
        .bind(response_body)
        .execute(pool)
        .await
        .expect("insert request detail");
    }

    #[tokio::test]
    async fn details_are_scoped_to_the_key_that_wrote_them() {
        let pool = pool().await;
        let first = insert_key(&pool, "first", None, None).await;
        let second = insert_key(&pool, "second", None, None).await;
        let request_id = "shared-request-id";
        insert_detail(&pool, request_id, first, "request-first", "response-first").await;
        insert_detail(
            &pool,
            request_id,
            second,
            "request-second",
            "response-second",
        )
        .await;
        let first_log =
            insert_usage(&pool, request_id, Some(first), Some(first), "model-first").await;
        let second_log = insert_usage(
            &pool,
            request_id,
            Some(second),
            Some(second),
            "model-second",
        )
        .await;

        let first_row = find_by_id(&pool, first_log)
            .await
            .expect("find first")
            .expect("first row");
        let second_row = find_by_id(&pool, second_log)
            .await
            .expect("find second")
            .expect("second row");
        assert_eq!(first_row.api_key_id, Some(first));
        assert_eq!(first_row.model, "model-first");
        assert_eq!(second_row.api_key_id, Some(second));
        assert_eq!(second_row.model, "model-second");

        assert_eq!(
            detail_bodies(&pool, first_log).await.expect("first bodies"),
            Some((
                Some("request-first".to_string()),
                Some("response-first".to_string())
            )),
            "the first key must not receive the second key's bodies"
        );
        assert_eq!(
            detail_bodies(&pool, second_log)
                .await
                .expect("second bodies"),
            Some((
                Some("request-second".to_string()),
                Some("response-second".to_string())
            )),
            "the second key must not receive the first key's bodies"
        );
    }

    #[tokio::test]
    async fn deleted_key_details_do_not_fall_back_to_another_key() {
        let pool = pool().await;
        let deleted = insert_key(&pool, "deleted", None, None).await;
        let live = insert_key(&pool, "live", None, None).await;
        let request_id = "reused-request-id";
        let history_log = insert_usage(
            &pool,
            request_id,
            Some(deleted),
            Some(deleted),
            "model-history",
        )
        .await;

        // Deleting the key nulls `usage_logs.api_key_id` but leaves the usage
        // row and its `detail_api_key_id` behind. Recreate the detail row as a
        // historical record (written before the cleanup trigger existed) so the
        // reader has something to return for the deleted owner.
        sqlx::query("DELETE FROM api_keys WHERE id = ?")
            .bind(deleted)
            .execute(&pool)
            .await
            .expect("delete api key");
        insert_detail(
            &pool,
            request_id,
            deleted,
            "history-request",
            "history-response",
        )
        .await;
        insert_detail(&pool, request_id, live, "live-request", "live-response").await;

        let row = find_by_id(&pool, history_log)
            .await
            .expect("find history")
            .expect("history row");
        assert_eq!(
            row.api_key_id, None,
            "deleting the key must clear the usage row's foreign key"
        );

        assert_eq!(
            detail_bodies(&pool, history_log)
                .await
                .expect("history bodies"),
            Some((
                Some("history-request".to_string()),
                Some("history-response".to_string())
            )),
            "the historical detail must be found by its deleted key id, not the live key"
        );
    }
}
