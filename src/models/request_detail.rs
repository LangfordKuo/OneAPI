use sqlx::SqlitePool;

pub const MAX_DETAIL_BYTES: usize = 64 * 1024;

pub async fn insert(
    pool: &SqlitePool,
    request_id: &str,
    request_body: &str,
    response_body: &str,
    created_at: i64,
) -> sqlx::Result<()> {
    let request_body = truncate(request_body);
    let response_body = truncate(response_body);

    sqlx::query(
        "INSERT INTO request_details (request_id, request_body, response_body, created_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(request_id) DO UPDATE SET
            request_body = excluded.request_body,
            response_body = excluded.response_body,
            created_at = excluded.created_at",
    )
    .bind(request_id)
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
    use super::*;

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
