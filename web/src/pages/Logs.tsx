import { useMemo, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Drawer,
  Input,
  Select,
  Space,
  Spin,
  Table,
  Tag,
  Tooltip,
  Typography,
} from 'antd';
import type { TableProps } from 'antd';
import dayjs from 'dayjs';
import { api } from '../api/client';
import type { ApiKey, Channel, LogDetail, LogEntry, LogList, ProtocolName } from '../api/types';

const RANGE_OPTIONS = [
  { value: '1h', label: '最近 1 小时' },
  { value: '24h', label: '最近 24 小时' },
  { value: '7d', label: '最近 7 天' },
  { value: '30d', label: '最近 30 天' },
  { value: 'all', label: '全部' },
];

const STATUS_OPTIONS = [
  { value: 'all', label: '全部状态' },
  { value: 'success', label: '成功' },
  { value: 'error', label: '失败' },
];

const PROTOCOL_OPTIONS = [
  { value: 'all', label: '全部协议' },
  { value: 'chat', label: 'Chat' },
  { value: 'responses', label: 'Responses' },
  { value: 'anthropic', label: 'Anthropic' },
];

const SOURCE_OPTIONS = [
  { value: 'all', label: '全部来源' },
  { value: 'gateway', label: '网关' },
  { value: 'playground', label: 'Playground' },
];

const STREAM_OPTIONS = [
  { value: 'all', label: '全部' },
  { value: 'true', label: '流式' },
  { value: 'false', label: '非流式' },
];

interface Filters {
  range: string;
  model: string;
  requestId: string;
  channelId?: number;
  apiKeyId?: number;
  status: string;
  protocol: string;
  source: string;
  stream: 'all' | 'true' | 'false';
}

const INITIAL_FILTERS: Filters = {
  range: '24h',
  model: '',
  requestId: '',
  status: 'all',
  protocol: 'all',
  source: 'all',
  stream: 'all',
};

function protocolLabel(protocol: ProtocolName): string {
  return protocol === 'chat' ? 'Chat' : protocol === 'responses' ? 'Responses' : 'Anthropic';
}

function formatTime(timestamp: number): string {
  return dayjs(timestamp).format('YYYY-MM-DD HH:mm:ss');
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? '-' : value.toLocaleString('zh-CN');
}

function formatLatency(value: number | null | undefined): string {
  return value == null ? '-' : `${value} ms`;
}

function formatTokensPerSecond(value: number | null | undefined): string {
  return value == null ? '-' : `${value.toFixed(1)} tok/s`;
}

function statusTag(entry: LogEntry) {
  return entry.status_code >= 400 ? (
    <Tag color="red">{entry.status_code}</Tag>
  ) : (
    <Tag color="green">{entry.status_code}</Tag>
  );
}

export default function Logs() {
  const [filters, setFilters] = useState<Filters>(INITIAL_FILTERS);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [selected, setSelected] = useState<LogEntry | null>(null);

  const update = <K extends keyof Filters>(key: K, value: Filters[K]) => {
    setFilters((previous) => ({ ...previous, [key]: value }));
    setPage(1);
  };

  const channelsQuery = useQuery({
    queryKey: ['channels'],
    queryFn: () => api.get<Channel[]>('/api/channels'),
  });
  const keysQuery = useQuery({
    queryKey: ['keys'],
    queryFn: () => api.get<ApiKey[]>('/api/keys'),
  });

  const path = useMemo(() => {
    const params = new URLSearchParams();
    params.set('range', filters.range);
    params.set('limit', String(pageSize));
    params.set('offset', String((page - 1) * pageSize));
    if (filters.model.trim()) params.set('model', filters.model.trim());
    if (filters.requestId.trim()) params.set('request_id', filters.requestId.trim());
    if (filters.channelId != null) params.set('channel_id', String(filters.channelId));
    if (filters.apiKeyId != null) params.set('api_key_id', String(filters.apiKeyId));
    if (filters.status !== 'all') params.set('status', filters.status);
    if (filters.protocol !== 'all') params.set('protocol', filters.protocol);
    if (filters.source !== 'all') params.set('source', filters.source);
    if (filters.stream !== 'all') params.set('stream', filters.stream);
    return `/api/logs?${params.toString()}`;
  }, [filters, page, pageSize]);

  const logsQuery = useQuery({
    queryKey: ['logs', path],
    queryFn: () => api.get<LogList>(path),
  });

  const detailQuery = useQuery({
    queryKey: ['log-detail', selected?.request_id],
    queryFn: () => api.get<LogDetail>(`/api/logs/${selected?.request_id}`),
    enabled: selected != null,
  });

  const columns: TableProps<LogEntry>['columns'] = [
    {
      title: '时间',
      dataIndex: 'created_at',
      width: 170,
      render: (value: number) => formatTime(value),
    },
    {
      title: '请求 ID',
      dataIndex: 'request_id',
      width: 180,
      render: (value: string) => (
        <Tooltip title={value}>
          <Typography.Text copyable={{ text: value }} ellipsis style={{ maxWidth: 150 }}>
            {value.slice(0, 12)}…
          </Typography.Text>
        </Tooltip>
      ),
    },
    { title: '模型', dataIndex: 'model', width: 150, ellipsis: true },
    {
      title: '协议',
      key: 'protocol',
      width: 150,
      render: (_, entry) =>
        `${protocolLabel(entry.inbound_protocol)} → ${protocolLabel(entry.upstream_protocol)}`,
    },
    {
      title: '来源',
      dataIndex: 'source',
      width: 100,
      render: (value: string) =>
        value === 'playground' ? <Tag color="purple">Playground</Tag> : <Tag>网关</Tag>,
    },
    {
      title: '渠道',
      key: 'channel',
      width: 140,
      ellipsis: true,
      render: (_, entry) => entry.channel_name ?? (entry.channel_id ? `#${entry.channel_id}` : '-'),
    },
    {
      title: 'API Key',
      key: 'key',
      width: 130,
      ellipsis: true,
      render: (_, entry) =>
        entry.api_key_name ?? (entry.api_key_id ? `#${entry.api_key_id}` : '-'),
    },
    { title: '状态', key: 'status', width: 80, render: (_, entry) => statusTag(entry) },
    {
      title: '流式',
      dataIndex: 'stream',
      width: 70,
      render: (value: boolean) => (value ? <Tag color="blue">是</Tag> : <Tag>否</Tag>),
    },
    {
      title: 'Tokens',
      key: 'tokens',
      width: 110,
      render: (_, entry) => (
        <Tooltip
          title={`输入 ${formatNumber(entry.prompt_tokens)} / 输出 ${formatNumber(
            entry.completion_tokens,
          )}`}
        >
          {formatNumber(entry.total_tokens)}
        </Tooltip>
      ),
    },
    {
      title: '延迟',
      key: 'latency',
      width: 130,
      render: (_, entry) => (
        <Tooltip title={`首 token ${formatLatency(entry.first_token_ms)}`}>
          {formatLatency(entry.latency_ms)}
        </Tooltip>
      ),
    },
    {
      title: '速度',
      key: 'speed',
      width: 100,
      render: (_, entry) => (
        <Tooltip title="输出 tokens ÷ 生成耗时；流式已扣除首 token 等待">
          {formatTokensPerSecond(entry.tokens_per_second)}
        </Tooltip>
      ),
    },
    { title: '尝试', dataIndex: 'attempts', width: 70 },
    {
      title: '错误',
      key: 'error',
      ellipsis: true,
      render: (_, entry) =>
        entry.error_message ? (
          <Tooltip title={entry.error_message}>
            <Typography.Text type="danger" ellipsis style={{ maxWidth: 180 }}>
              {entry.error_type ?? 'error'}: {entry.error_message}
            </Typography.Text>
          </Tooltip>
        ) : (
          '-'
        ),
    },
  ];

  return (
    <>
      <Card style={{ marginBottom: 16 }}>
        <Space wrap size={[8, 8]}>
          <Select
            value={filters.range}
            options={RANGE_OPTIONS}
            style={{ width: 140 }}
            onChange={(value) => update('range', value)}
          />
          <Input
            allowClear
            placeholder="模型"
            value={filters.model}
            style={{ width: 160 }}
            onChange={(event) => update('model', event.target.value)}
          />
          <Input
            allowClear
            placeholder="请求 ID"
            value={filters.requestId}
            style={{ width: 220 }}
            onChange={(event) => update('requestId', event.target.value)}
          />
          <Select
            allowClear
            showSearch
            optionFilterProp="label"
            placeholder="渠道"
            value={filters.channelId}
            style={{ width: 160 }}
            options={(channelsQuery.data ?? []).map((channel) => ({
              value: channel.id,
              label: channel.name,
            }))}
            onChange={(value) => update('channelId', value)}
          />
          <Select
            allowClear
            showSearch
            optionFilterProp="label"
            placeholder="API Key"
            value={filters.apiKeyId}
            style={{ width: 160 }}
            options={(keysQuery.data ?? []).map((key) => ({
              value: key.id,
              label: key.name,
            }))}
            onChange={(value) => update('apiKeyId', value)}
          />
          <Select
            value={filters.status}
            options={STATUS_OPTIONS}
            style={{ width: 120 }}
            onChange={(value) => update('status', value)}
          />
          <Select
            value={filters.protocol}
            options={PROTOCOL_OPTIONS}
            style={{ width: 130 }}
            onChange={(value) => update('protocol', value)}
          />
          <Select
            value={filters.source}
            options={SOURCE_OPTIONS}
            style={{ width: 130 }}
            onChange={(value) => update('source', value)}
          />
          <Select
            value={filters.stream}
            options={STREAM_OPTIONS}
            style={{ width: 110 }}
            onChange={(value) => update('stream', value)}
          />
          <Button
            onClick={() => {
              setFilters(INITIAL_FILTERS);
              setPage(1);
            }}
          >
            重置
          </Button>
        </Space>
      </Card>

      {logsQuery.error ? (
        <Alert
          type="error"
          showIcon
          message="加载日志失败"
          description={
            logsQuery.error instanceof Error ? logsQuery.error.message : String(logsQuery.error)
          }
        />
      ) : (
        <Table<LogEntry>
          rowKey="id"
          columns={columns}
          dataSource={logsQuery.data?.items ?? []}
          loading={logsQuery.isLoading}
          size="middle"
          scroll={{ x: 1620 }}
          rowClassName={() => 'clickable-row'}
          onRow={(record) => ({ onClick: () => setSelected(record) })}
          pagination={{
            current: page,
            pageSize,
            total: logsQuery.data?.total ?? 0,
            showSizeChanger: true,
            pageSizeOptions: [20, 50, 100],
            showTotal: (total) => `共 ${total} 条`,
            onChange: (nextPage, nextPageSize) => {
              setPage(nextPage);
              setPageSize(nextPageSize);
            },
          }}
        />
      )}

      <Drawer
        title="请求详情"
        width={720}
        open={selected != null}
        onClose={() => setSelected(null)}
      >
        {detailQuery.isLoading ? (
          <div className="center-shell">
            <Spin />
          </div>
        ) : detailQuery.error ? (
          <Alert
            type="error"
            showIcon
            message="加载详情失败"
            description={
              detailQuery.error instanceof Error
                ? detailQuery.error.message
                : String(detailQuery.error)
            }
          />
        ) : detailQuery.data ? (
          <>
            <Descriptions column={2} size="small" bordered>
              <Descriptions.Item label="请求 ID" span={2}>
                {detailQuery.data.request_id}
              </Descriptions.Item>
              <Descriptions.Item label="时间">
                {formatTime(detailQuery.data.created_at)}
              </Descriptions.Item>
              <Descriptions.Item label="来源">{detailQuery.data.source}</Descriptions.Item>
              <Descriptions.Item label="入站协议">
                {protocolLabel(detailQuery.data.inbound_protocol)}
              </Descriptions.Item>
              <Descriptions.Item label="上游协议">
                {protocolLabel(detailQuery.data.upstream_protocol)}
              </Descriptions.Item>
              <Descriptions.Item label="客户端模型">{detailQuery.data.model}</Descriptions.Item>
              <Descriptions.Item label="上游模型">
                {detailQuery.data.upstream_model ?? '-'}
              </Descriptions.Item>
              <Descriptions.Item label="渠道">
                {detailQuery.data.channel_name ??
                  (detailQuery.data.channel_id ? `#${detailQuery.data.channel_id}` : '-')}
              </Descriptions.Item>
              <Descriptions.Item label="API Key">
                {detailQuery.data.api_key_name ??
                  (detailQuery.data.api_key_id ? `#${detailQuery.data.api_key_id}` : '-')}
              </Descriptions.Item>
              <Descriptions.Item label="状态码">
                {statusTag(detailQuery.data)}
              </Descriptions.Item>
              <Descriptions.Item label="流式">
                {detailQuery.data.stream ? '是' : '否'}
              </Descriptions.Item>
              <Descriptions.Item label="尝试次数">
                {detailQuery.data.attempts}
              </Descriptions.Item>
              <Descriptions.Item label="延迟">
                {formatLatency(detailQuery.data.latency_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="首 token">
                {formatLatency(detailQuery.data.first_token_ms)}
              </Descriptions.Item>
              <Descriptions.Item label="生成速度">
                {formatTokensPerSecond(detailQuery.data.tokens_per_second)}
              </Descriptions.Item>
              <Descriptions.Item label="输入 / 输出 / 总计">
                {formatNumber(detailQuery.data.prompt_tokens)} /{' '}
                {formatNumber(detailQuery.data.completion_tokens)} /{' '}
                {formatNumber(detailQuery.data.total_tokens)}
              </Descriptions.Item>
              <Descriptions.Item label="缓存读取 / 缓存写入 / 推理">
                {formatNumber(detailQuery.data.cached_tokens)} /{' '}
                {formatNumber(detailQuery.data.cache_creation_tokens)} /{' '}
                {formatNumber(detailQuery.data.reasoning_tokens)}
              </Descriptions.Item>
              {detailQuery.data.error_message ? (
                <Descriptions.Item label="错误" span={2}>
                  <Typography.Text type="danger">
                    {detailQuery.data.error_type ?? 'error'}: {detailQuery.data.error_message}
                  </Typography.Text>
                </Descriptions.Item>
              ) : null}
            </Descriptions>

            <Typography.Title level={5} style={{ marginTop: 24 }}>
              请求 / 响应体
            </Typography.Title>
            {detailQuery.data.request_body || detailQuery.data.response_body ? (
              <>
                {detailQuery.data.request_body ? (
                  <pre className="code-block">{detailQuery.data.request_body}</pre>
                ) : null}
                {detailQuery.data.response_body ? (
                  <pre className="code-block">{detailQuery.data.response_body}</pre>
                ) : null}
              </>
            ) : (
              <Alert
                type="info"
                showIcon
                message="未记录请求/响应体"
                description="可在「设置 → 记录请求/响应体」中开启；开启后单条最多保存 64KB。"
              />
            )}
          </>
        ) : null}
      </Drawer>
    </>
  );
}
