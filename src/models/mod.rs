pub mod admin;
pub mod api_key;
pub mod channel;
pub mod response;
pub mod session;
pub mod setting;
pub mod usage_log;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
