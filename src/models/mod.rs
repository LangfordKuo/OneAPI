pub mod admin;
pub mod api_key;
pub mod channel;
pub mod session;
pub mod setting;

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
