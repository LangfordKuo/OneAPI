use sqlx::{FromRow, QueryBuilder, Sqlite, SqlitePool};

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
    l.completion_tokens, l.total_tokens, l.cached_tokens, l.reasoning_tokens,
    l.latency_ms, l.first_token_ms, l.created_at,
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
            .push(" AND l.model LIKE ")
            .push_bind(format!("%{model}%"));
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
            .push(" AND l.request_id LIKE ")
            .push_bind(format!("%{request_id}%"));
    }
}

pub async fn list(
    pool: &SqlitePool,
    filters: &LogFilters,
    limit: i64,
    offset: i64,
) -> sqlx::Result<(Vec<UsageLogRow>, i64)> {
    let mut query = QueryBuilder::<Sqlite>::new(SELECT);
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

    let mut count = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM usage_logs l WHERE 1=1");
    push_filters(&mut count, filters);
    let total: i64 = count.build_query_scalar().fetch_one(pool).await?;

    Ok((rows, total))
}

pub async fn find_by_request_id(
    pool: &SqlitePool,
    request_id: &str,
) -> sqlx::Result<Option<UsageLogRow>> {
    let query = format!("{SELECT} WHERE l.request_id = ? ORDER BY l.id DESC LIMIT 1");
    sqlx::query_as(&query)
        .bind(request_id)
        .fetch_optional(pool)
        .await
}

pub async fn detail_bodies(
    pool: &SqlitePool,
    request_id: &str,
) -> sqlx::Result<Option<(Option<String>, Option<String>)>> {
    sqlx::query_as("SELECT request_body, response_body FROM request_details WHERE request_id = ?")
        .bind(request_id)
        .fetch_optional(pool)
        .await
}
