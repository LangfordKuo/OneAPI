use sqlx::SqlitePool;

pub const MAX_DETAIL_BYTES: usize = 64 * 1024;

/// Store the request/response bodies for one request. `api_key_id` uses `0`
/// for playground requests and legacy rows, so two API keys that happen to use
/// the same client-supplied request id cannot overwrite each other.
pub async fn insert(
    pool: &SqlitePool,
    request_id: &str,
    api_key_id: i64,
    request_body: &str,
    response_body: &str,
    created_at: i64,
) -> sqlx::Result<()> {
    let request_body = truncate(request_body);
    let response_body = truncate(response_body);

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
    .execute(pool)
    .await?;

    Ok(())
}

fn truncate(value: &str) -> String {
    if value.len() <= MAX_DETAIL_BYTES {
        return value.to_string();
    }

    let mut end = MAX_DETAIL_BYTES;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n...[truncated]", &value[..end])
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;
    use tempfile::TempDir;

    use super::*;

    async fn pool() -> (TempDir, SqlitePool) {
        let temp = TempDir::new().expect("temp dir");
        let config = crate::config::Config {
            addr: "127.0.0.1:0".parse().expect("address"),
            data_dir: temp.path().to_path_buf(),
            database_url: None,
            database_path: temp.path().join("test.db"),
            log_level: "error".to_string(),
            cookie_secure: false,
            trust_proxy: false,
            database_pool_size: 5,
        };
        let pool = crate::db::connect(&config).await.expect("database");
        (temp, pool)
    }

    #[tokio::test]
    async fn details_are_scoped_by_api_key() {
        let (_temp, pool) = pool().await;
        insert(&pool, "shared", 1, "first", "response-one", 1)
            .await
            .expect("first");
        insert(&pool, "shared", 2, "second", "response-two", 2)
            .await
            .expect("second");

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM request_details WHERE request_id = 'shared'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(count, 2);

        let key_one: (String, String) = sqlx::query_as(
            "SELECT request_body, response_body FROM request_details
             WHERE request_id = 'shared' AND api_key_id = 1",
        )
        .fetch_one(&pool)
        .await
        .expect("key one");
        assert_eq!(key_one.0, "first");
        assert_eq!(key_one.1, "response-one");
    }

    #[tokio::test]
    async fn repeated_request_overwrites_the_same_key_row() {
        let (_temp, pool) = pool().await;
        insert(&pool, "same", 0, "old", "old-response", 1)
            .await
            .expect("first");
        insert(&pool, "same", 0, "new", "new-response", 2)
            .await
            .expect("second");

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM request_details WHERE request_id = 'same'")
                .fetch_one(&pool)
                .await
                .expect("count");
        assert_eq!(count, 1);
        let body: String = sqlx::query_scalar(
            "SELECT request_body FROM request_details WHERE request_id = 'same'",
        )
        .fetch_one(&pool)
        .await
        .expect("row");
        assert_eq!(body, "new");
    }

    #[test]
    fn truncates_on_utf8_boundary() {
        let value = "汉".repeat(MAX_DETAIL_BYTES);
        let truncated = truncate(&value);
        assert!(truncated.len() <= MAX_DETAIL_BYTES + 20);
        assert!(truncated.ends_with("...[truncated]"));
        assert!(truncated
            .trim_end_matches("\n...[truncated]")
            .chars()
            .all(|ch| ch == '汉'));
    }
}
