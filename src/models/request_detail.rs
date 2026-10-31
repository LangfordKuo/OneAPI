use sqlx::{Executor, Sqlite};

pub const MAX_DETAIL_BYTES: usize = 64 * 1024;

/// Store the request/response bodies for one request. `api_key_id` uses `0`
/// for playground requests and legacy rows, so two API keys that happen to use
/// the same client-supplied request id cannot overwrite each other.
pub async fn insert<'e, E>(
    executor: E,
    request_id: &str,
    api_key_id: i64,
    request_body: &str,
    response_body: &str,
    created_at: i64,
) -> sqlx::Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
    let request_body = truncate_detail(request_body);
    let response_body = truncate_detail(response_body);

    sqlx::query(
        "INSERT INTO request_details
         (request_id, api_key_id, request_body, response_body, created_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(request_id, api_key_id) DO UPDATE SET
            request_body = excluded.request_body,
            response_body = excluded.response_body,
            created_at = excluded.created_at",
    )
    .bind(request_id)
    .bind(api_key_id)
    .bind(request_body)
    .bind(response_body)
    .bind(created_at)
    .execute(executor)
    .await?;

    Ok(())
}

/// Truncate a body to `MAX_DETAIL_BYTES` without splitting a UTF-8 character.
/// Also applied on the capture side so oversized bodies are not copied into the
/// write queue in full.
pub(crate) fn truncate_detail(value: &str) -> String {
    if value.len() <= MAX_DETAIL_BYTES {
        return value.to_string();
    }

    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated]", &value[..end])
}
