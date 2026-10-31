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

export interface PlaygroundTool {
  key: string;
  id: string;
  name: string;
  arguments: string;
}

export interface PlaygroundUsage {
  input?: number;
  output?: number;
  total?: number;
}

export type PlaygroundMessageStatus = 'done' | 'streaming' | 'error' | 'stopped';

export interface PlaygroundMessageMeta {
  channel?: string | null;
  upstream_model?: string | null;
  model?: string | null;
  response_id?: string | null;
  stop_reason?: string | null;
  usage?: PlaygroundUsage | null;
  elapsed_ms?: number | null;
  first_token_ms?: number | null;
  reasoning_ms?: number | null;
}

export interface PlaygroundMessage {
  role: 'user' | 'assistant';
  content: string;
  reasoning: string;
  tools: PlaygroundTool[];
  status: PlaygroundMessageStatus;
  error: string;
  meta: PlaygroundMessageMeta;
}

export interface PlaygroundSessionSummary {
  id: string;
  title: string;
  protocol: ProtocolName;
  model: string;
  created_at: number;
  updated_at: number;
  message_count: number;
}

export interface PlaygroundSession {
  id: string;
  title: string;
  protocol: ProtocolName;
  model: string;
  system: string;
  stream: boolean;
  max_tokens: number;
  created_at: number;
  updated_at: number;
  messages: PlaygroundMessage[];
}

export interface PlaygroundSessionsOutput {
  items: PlaygroundSessionSummary[];
}

export interface PlaygroundSessionOutput {
  session: PlaygroundSession;
}

export interface SiteInfo {
  site_name: string;
  version: string;
}

export interface SystemInfo {
  name: string;
  version: string;
  uptime_seconds: number;
  database: string;
  data_dir: string;
  database_bytes: number;
  wal_bytes: number;
  last_backup_at: number | null;
  usage_queue_depth: number;
  /** Responses objects skipped by storage limits or id conflicts. */
  response_store_rejected?: number;
  /** Request details dropped after the detail queue exceeded its budget. */
  detail_dropped?: number;
  /** Bytes currently held by queued request details. */
  detail_queue_bytes?: number;
  cooling_channels: number;
  failovers_24h: number;
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
