export type ChannelKind = 'openai_chat' | 'openai_responses' | 'anthropic';
export type ProtocolName = 'chat' | 'responses' | 'anthropic';
export type ChannelProtocolMode = 'native' | 'convert';

export interface ChannelProtocol {
  protocol: ChannelKind;
  mode: ChannelProtocolMode;
  target: ChannelKind | null;
  base_url: string;
  auth_header: 'bearer' | 'x-api-key';
  api_key_masked: string;
  models: string[];
  model_mapping: Record<string, string>;
  extra_headers: Record<string, string>;
}

export interface Channel {
  id: number;
  name: string;
  kind: ChannelKind;
  kinds: ChannelKind[];
  fallback_kind: ChannelKind;
  base_url: string;
  auth_header: 'bearer' | 'x-api-key';
  api_key_masked: string;
  models: string[];
  model_mapping: Record<string, string>;
  extra_headers: Record<string, string>;
  protocols: ChannelProtocol[];
  proxy_url: string | null;
  priority: number;
  weight: number;
  timeout_ms: number | null;
  enabled: boolean;
  last_error: string | null;
  last_error_at: number | null;
  disabled_until: number | null;
  consecutive_failures: number;
  created_at: number;
  updated_at: number;
}

export interface ApiKey {
  id: number;
  name: string;
  key_prefix: string;
  enabled: boolean;
  expires_at: number | null;
  last_used_at: number | null;
  allowed_channel_ids: number[];
  allowed_models: string[];
  token_limit: number | null;
  token_usage: number;
  daily_token_limit: number | null;
  daily_token_usage: number;
  created_at: number;
  key?: string;
}

export interface Settings {
  site_name: string;
  site_domain: string;
  default_max_tokens: number;
  nonstream_timeout_seconds: number;
  stream_idle_timeout_seconds: number;
  usage_log_retention_days: number;
  request_detail_retention_days: number;
  response_retention_days: number;
  response_storage_per_key_mb: number;
  response_storage_total_mb: number;
  store_request_bodies: boolean;
  update_source: 'github' | 'ghproxy';
  client_ip_header: string;
}

export interface Admin {
  username: string;
  created_at: number;
}

export interface ChannelTestResult {
  ok: boolean;
  status: number;
  latency_ms: number;
  message: string;
}

export interface ModelFetchResult {
  ok: boolean;
  status: number;
  latency_ms: number;
  models: string[];
  message: string;
}

export interface LogEntry {
  id: number;
  request_id: string;
  source: 'gateway' | 'playground';
  api_key_id: number | null;
  channel_id: number | null;
  model: string;
  upstream_model: string | null;
  inbound_protocol: ProtocolName;
  upstream_protocol: ProtocolName;
  stream: boolean;
  status_code: number;
  attempts: number;
  error_type: string | null;
  error_message: string | null;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  total_tokens: number | null;
  cached_tokens: number | null;
  cache_creation_tokens: number | null;
  reasoning_tokens: number | null;
  latency_ms: number;
  first_token_ms: number | null;
  tokens_per_second: number | null;
  created_at: number;
  api_key_name: string | null;
  channel_name: string | null;
}

export interface LogList {
  items: LogEntry[];
  /** Null when the request skipped the COUNT(*) scan (deep pages). */
  total: number | null;
  limit: number;
  offset: number;
}

export interface LogDetail extends LogEntry {
  request_body: string | null;
  response_body: string | null;
}

export interface DashboardSummary {
  range: string;
  requests: number;
  successes: number;
  errors: number;
  success_rate: number;
  total_tokens: number;
  prompt_tokens: number;
  completion_tokens: number;
  cached_tokens: number;
  reasoning_tokens: number;
  avg_latency_ms: number;
  avg_first_token_ms: number | null;
  active_channels: number;
  active_keys: number;
}

export interface TimeseriesPoint {
  timestamp: number;
  requests: number;
  errors: number;
  total_tokens: number;
  avg_latency_ms: number;
}

export interface Timeseries {
  range: string;
  bucket_seconds: number;
  points: TimeseriesPoint[];
}

export interface BreakdownItem {
  key: string;
  label: string;
  requests: number;
  total_tokens: number;
  errors: number;
  avg_latency_ms: number;
}

export interface Breakdown {
  dimension: string;
  items: BreakdownItem[];
}

export interface ModelsOutput {
  models: string[];
}

export interface SiteInfo {
  site_name: string;
  version: string;
}

export interface SetupStatus {
  initialized: boolean;
  version: string;
  addr?: string;
  data_dir?: string;
  database?: string;
  env_path?: string;
}

export interface SetupResult {
  username: string;
  env_written: boolean;
  env_path: string;
  message: string;
}

export interface UpdateInfo {
  current_version: string;
  latest_version: string;
  update_available: boolean;
  release_notes: string | null;
  published_at: string | null;
  html_url: string | null;
  asset_name: string | null;
  asset_url: string | null;
  checksums_url: string | null;
  source: 'github' | 'ghproxy';
}

export interface UpdateApplyResult {
  updated_to: string;
  asset_name: string;
  message: string;
}

export interface BackupFile {
  name: string;
  bytes: number;
  /** Unix milliseconds. */
  modified_at: number;
}

export interface BackupListOutput {
  items: BackupFile[];
}

export interface BackupSnapshotResult {
  ok: boolean;
  file: string;
  bytes: number;
  message: string;
}
