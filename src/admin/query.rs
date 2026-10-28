use crate::{error::AppError, models::now_ms};

pub const DEFAULT_RANGE: &str = "24h";

/// Parse a range label into an inclusive lower timestamp. `all` means no cutoff.
pub fn since(range: Option<&str>) -> Result<Option<i64>, AppError> {
    let label = range.unwrap_or(DEFAULT_RANGE);
    let millis = match label {
        "1h" => 3_600_000,
        "24h" => 86_400_000,
        "7d" => 604_800_000,
        "30d" => 2_592_000_000,
        "all" => return Ok(None),
        other => return Err(AppError::BadRequest(format!("unsupported range `{other}`"))),
    };

    Ok(Some(now_ms() - millis))
}

/// Dashboard ranges must map to a finite bucket size.
pub fn window(range: Option<&str>) -> Result<(i64, i64), AppError> {
    let label = range.unwrap_or(DEFAULT_RANGE);
    let (millis, bucket_seconds) = match label {
        // A one-hour window needs minute buckets; an hourly bucket would
        // produce only one or two points and an empty-looking chart.
        "1h" => (3_600_000, 60),
        "24h" => (86_400_000, 3_600),
        "7d" => (604_800_000, 86_400),
        "30d" => (2_592_000_000, 86_400),
        other => {
            return Err(AppError::BadRequest(format!(
                "unsupported dashboard range `{other}`"
            )))
        }
    };

    Ok((now_ms() - millis, bucket_seconds))
}

#[cfg(test)]
mod tests {
    use super::{since, window, DEFAULT_RANGE};
    use crate::models::now_ms;

    #[test]
    fn one_hour_uses_minute_buckets() {
        let (since, bucket) = window(Some("1h")).expect("1h window");
        assert_eq!(bucket, 60);
        assert!(since <= now_ms() - 3_500_000);
    }

    #[test]
    fn longer_ranges_keep_hour_and_day_buckets() {
        assert_eq!(window(Some("24h")).expect("24h").1, 3_600);
        assert_eq!(window(Some("7d")).expect("7d").1, 86_400);
        assert_eq!(window(Some("30d")).expect("30d").1, 86_400);
    }

    #[test]
    fn dashboard_rejects_all_and_unknown_ranges() {
        assert!(window(Some("all")).is_err());
        assert!(window(Some("2h")).is_err());
    }

    #[test]
    fn log_ranges_allow_all() {
        assert_eq!(since(Some("all")).expect("all"), None);
        let default = since(None).expect("default").expect("bounded");
        let explicit = since(Some(DEFAULT_RANGE)).expect("24h").expect("bounded");
        assert!((default - explicit).abs() < 1_000);
    }
}
