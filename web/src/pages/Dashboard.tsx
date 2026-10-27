import { lazy, Suspense, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  Alert,
  Card,
  Col,
  Descriptions,
  Row,
  Select,
  Space,
  Statistic,
  Table,
  Tag,
  Typography,
} from 'antd';
import type { TableProps } from 'antd';
import dayjs from 'dayjs';
import { api } from '../api/client';
import type {
  Breakdown,
  DashboardSummary,
  LogEntry,
  LogList,
  Timeseries,
} from '../api/types';

const DashboardCharts = lazy(() => import('../components/DashboardCharts'));

const RANGE_OPTIONS = [
  { value: '1h', label: '最近 1 小时' },
  { value: '24h', label: '最近 24 小时' },
  { value: '7d', label: '最近 7 天' },
  { value: '30d', label: '最近 30 天' },
];

interface SystemInfo {
  name: string;
  version: string;
  uptime_seconds: number;
  database: string;
  data_dir: string;
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) {
    return `${days} 天 ${hours} 小时`;
  }
  if (hours > 0) {
    return `${hours} 小时 ${minutes} 分`;
  }
  return `${minutes} 分`;
}

function formatNumber(value: number | null | undefined): string {
  return value == null ? '-' : value.toLocaleString('zh-CN');
}

export default function Dashboard() {
  const [range, setRange] = useState('24h');

  const systemQuery = useQuery({
    queryKey: ['system'],
    queryFn: () => api.get<SystemInfo>('/api/system'),
    refetchInterval: 30_000,
  });
  const summaryQuery = useQuery({
    queryKey: ['dashboard-summary', range],
    queryFn: () => api.get<DashboardSummary>(`/api/dashboard/summary?range=${range}`),
  });
  const timeseriesQuery = useQuery({
    queryKey: ['dashboard-timeseries', range],
    queryFn: () => api.get<Timeseries>(`/api/dashboard/timeseries?range=${range}`),
  });
  const modelsQuery = useQuery({
    queryKey: ['dashboard-models', range],
    queryFn: () =>
      api.get<Breakdown>(`/api/dashboard/breakdown?range=${range}&dimension=model`),
  });
  const channelsQuery = useQuery({
    queryKey: ['dashboard-channels', range],
    queryFn: () =>
      api.get<Breakdown>(`/api/dashboard/breakdown?range=${range}&dimension=channel`),
  });
  const errorsQuery = useQuery({
    queryKey: ['dashboard-errors', range],
    queryFn: () => api.get<LogList>(`/api/logs?range=${range}&status=error&limit=5&offset=0`),
  });

  const summary = summaryQuery.data;
  const errorColumns: TableProps<LogEntry>['columns'] = [
    {
      title: '时间',
      dataIndex: 'created_at',
      width: 170,
      render: (value: number) => dayjs(value).format('YYYY-MM-DD HH:mm:ss'),
    },
    { title: '模型', dataIndex: 'model', width: 160, ellipsis: true },
    {
      title: '状态',
      dataIndex: 'status_code',
      width: 90,
      render: (value: number) => <Tag color="red">{value}</Tag>,
    },
    {
      title: '错误',
      key: 'error',
      ellipsis: true,
      render: (_, entry) =>
        entry.error_message ? `${entry.error_type ?? 'error'}: ${entry.error_message}` : '-',
    },
  ];

  return (
    <>
      {systemQuery.error ? (
        <Alert
          type="error"
          showIcon
          message="无法连接后端"
          description={
            systemQuery.error instanceof Error
              ? systemQuery.error.message
              : String(systemQuery.error)
          }
          style={{ marginBottom: 16 }}
        />
      ) : null}

      <Space style={{ marginBottom: 16 }}>
        <Typography.Text type="secondary">统计范围</Typography.Text>
        <Select
          value={range}
          options={RANGE_OPTIONS}
          style={{ width: 150 }}
          onChange={setRange}
        />
      </Space>

      <Row gutter={[16, 16]}>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic title="请求数" value={summary?.requests ?? 0} />
          </Card>
        </Col>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic
              title="成功率"
              value={(summary?.success_rate ?? 0) * 100}
              precision={1}
              suffix="%"
            />
          </Card>
        </Col>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic
              title="错误数"
              value={summary?.errors ?? 0}
              valueStyle={{ color: (summary?.errors ?? 0) > 0 ? '#cf1322' : undefined }}
            />
          </Card>
        </Col>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic title="总 Tokens" value={formatNumber(summary?.total_tokens)} />
          </Card>
        </Col>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic
              title="平均延迟"
              value={summary ? Math.round(summary.avg_latency_ms) : 0}
              suffix="ms"
            />
          </Card>
        </Col>
        <Col xs={12} md={8} xl={4}>
          <Card loading={summaryQuery.isLoading}>
            <Statistic
              title="平均首 token"
              value={
                summary?.avg_first_token_ms == null
                  ? '-'
                  : Math.round(summary.avg_first_token_ms)
              }
              suffix={summary?.avg_first_token_ms == null ? undefined : 'ms'}
            />
          </Card>
        </Col>
      </Row>

      <div style={{ marginTop: 16 }}>
        <Suspense fallback={<Card loading style={{ minHeight: 320 }} />}>
          <DashboardCharts
            points={timeseriesQuery.data?.points ?? []}
            models={modelsQuery.data?.items ?? []}
            channels={channelsQuery.data?.items ?? []}
          />
        </Suspense>
      </div>

      <Card title="最近错误" style={{ marginTop: 16 }}>
        <Table<LogEntry>
          rowKey="id"
          size="small"
          columns={errorColumns}
          dataSource={errorsQuery.data?.items ?? []}
          loading={errorsQuery.isLoading}
          pagination={false}
          locale={{ emptyText: '当前范围没有错误请求' }}
        />
      </Card>

      <Card title="系统信息" style={{ marginTop: 16 }} loading={systemQuery.isLoading}>
        <Descriptions column={2} size="small">
          <Descriptions.Item label="名称">{systemQuery.data?.name}</Descriptions.Item>
          <Descriptions.Item label="版本">{systemQuery.data?.version}</Descriptions.Item>
          <Descriptions.Item label="运行时长">
            {systemQuery.data ? formatUptime(systemQuery.data.uptime_seconds) : '-'}
          </Descriptions.Item>
          <Descriptions.Item label="数据库">
            <Tag color={systemQuery.data?.database === 'ok' ? 'green' : 'red'}>
              {systemQuery.data?.database ?? '-'}
            </Tag>
          </Descriptions.Item>
          <Descriptions.Item label="数据目录" span={2}>
            {systemQuery.data?.data_dir ?? '-'}
          </Descriptions.Item>
          <Descriptions.Item label="活跃渠道">
            {summary?.active_channels ?? '-'}
          </Descriptions.Item>
          <Descriptions.Item label="活跃 Key">{summary?.active_keys ?? '-'}</Descriptions.Item>
        </Descriptions>
      </Card>
    </>
  );
}
