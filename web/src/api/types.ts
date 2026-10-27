export type ChannelKind = 'openai_chat' | 'openai_responses' | 'anthropic';

export interface Channel {
  id: number;
  name: string;
  kind: ChannelKind;
  base_url: string;
  auth_header: 'bearer' | 'x-api-key';
  api_key_masked: string;
  models: string[];
  model_mapping: Record<string, string>;
  extra_headers: Record<string, string>;
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
  created_at: number;
  key?: string;
}

export interface Settings {
  site_name: string;
  default_max_tokens: number;
  nonstream_timeout_seconds: number;
  stream_idle_timeout_seconds: number;
  usage_log_retention_days: number;
  response_retention_days: number;
  store_request_bodies: boolean;
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
