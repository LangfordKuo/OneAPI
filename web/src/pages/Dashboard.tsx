import { useQuery } from '@tanstack/react-query';
import {
  Activity,
  AlertTriangle,
  CheckCircle2,
  Cpu,
  Timer,
  Zap,
} from 'lucide-react';
import {
  lazy,
  Suspense,
  useState,
  type ComponentType,
  type ReactNode,
} from 'react';
import { api } from '@/api/client';
import type {
  Breakdown,
  DashboardSummary,
  LogList,
  Timeseries,
} from '@/api/types';
import { EmptyState } from '@/components/common/empty-state';
import { PageHeader } from '@/components/common/page-header';
import { StatusCodeBadge } from '@/components/common/badges';
import { formatDateTime, formatNumber, formatUptime } from '@/lib/format';
import { useLiveMetrics } from '@/hooks/useLiveMetrics';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';

const DashboardCharts = lazy(() => import('@/components/DashboardCharts'));

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

interface StatCardProps {
  title: string;
  value: string;
  icon: ComponentType<{ className?: string }>;
  danger?: boolean;
}

function StatCard({ title, value, icon: Icon, danger }: StatCardProps) {
  return (
    <Card>
      <CardContent className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <p className="truncate text-xs text-muted-foreground">{title}</p>
          <p className="truncate text-2xl font-semibold tabular-nums">
            {value}
          </p>
        </div>
        <div
          className={
            danger
              ? 'rounded-lg bg-destructive/10 p-2 text-destructive'
              : 'rounded-lg bg-muted p-2 text-muted-foreground'
          }
        >
          <Icon className="size-4" />
        </div>
      </CardContent>
    </Card>
  );
}

function InfoItem({ label, value }: { label: string; value: ReactNode }) {
  return (
    <div className="min-w-0 space-y-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd
        className="truncate text-sm"
        title={typeof value === 'string' ? value : undefined}
      >
        {value}
      </dd>
    </div>
  );
}

export default function Dashboard() {
  const [range, setRange] = useState('24h');
  const live = useLiveMetrics();

  const systemQuery = useQuery({
    queryKey: ['system'],
    queryFn: () => api.get<SystemInfo>('/api/system'),
    refetchInterval: 30_000,
  });
  const summaryQuery = useQuery({
    queryKey: ['dashboard-summary', range],
    queryFn: () =>
      api.get<DashboardSummary>(`/api/dashboard/summary?range=${range}`),
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
      api.get<Breakdown>(
        `/api/dashboard/breakdown?range=${range}&dimension=channel`,
      ),
  });
  const errorsQuery = useQuery({
    queryKey: ['dashboard-errors', range],
    queryFn: () =>
      api.get<LogList>(
        `/api/logs?range=${range}&status=error&limit=5&offset=0`,
      ),
  });

  const summary = summaryQuery.data;
  const errors = errorsQuery.data?.items ?? [];

  return (
    <>
      <PageHeader
        title="仪表盘"
        description="查看请求量、Token 消耗、延迟与渠道健康状态。"
        actions={
          <Select
            items={RANGE_OPTIONS}
            value={range}
            onValueChange={(value) => {
              if (value) {
                setRange(value);
              }
            }}
          >
            <SelectTrigger className="w-[150px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {RANGE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        }
      />

      {systemQuery.isError ? (
        <Alert variant="destructive" className="mb-4">
          <AlertTriangle />
          <AlertTitle>无法连接后端</AlertTitle>
          <AlertDescription>
            {systemQuery.error instanceof Error
              ? systemQuery.error.message
              : String(systemQuery.error)}
          </AlertDescription>
        </Alert>
      ) : null}

      <Card className="mb-4 overflow-hidden bg-gradient-to-br from-emerald-500/5 via-transparent to-transparent">
        <CardContent className="flex flex-wrap items-center justify-between gap-4">
          <div className="flex items-center gap-3">
            <div className="rounded-xl bg-emerald-500/10 p-3 text-emerald-600 dark:text-emerald-400">
              <Activity className="size-5" />
            </div>
            <div>
              <p className="text-xs text-muted-foreground">全站实时输出</p>
              <p className="text-3xl font-semibold tabular-nums">
                {live.global_tok_s.toFixed(1)}
                <span className="ml-1.5 text-sm font-normal text-muted-foreground">
                  tok/s
                </span>
              </p>
            </div>
          </div>
          <Badge
            variant="outline"
            className={
              live.global_tok_s > 0
                ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                : 'text-muted-foreground'
            }
          >
            最近 1 秒实测
          </Badge>
        </CardContent>
      </Card>

      <div className="grid grid-cols-2 gap-4 md:grid-cols-3 xl:grid-cols-6">
        <StatCard
          title="请求数"
          value={summary ? formatNumber(summary.requests) : '—'}
          icon={Activity}
        />
        <StatCard
          title="成功率"
          value={
            summary ? `${(summary.success_rate * 100).toFixed(1)}%` : '—'
          }
          icon={CheckCircle2}
        />
        <StatCard
          title="错误数"
          value={summary ? formatNumber(summary.errors) : '—'}
          icon={AlertTriangle}
          danger={(summary?.errors ?? 0) > 0}
        />
        <StatCard
          title="总 Tokens"
          value={summary ? formatNumber(summary.total_tokens) : '—'}
          icon={Cpu}
        />
        <StatCard
          title="平均延迟"
          value={
            summary ? `${Math.round(summary.avg_latency_ms)} ms` : '—'
          }
          icon={Timer}
        />
        <StatCard
          title="平均首 Token"
          value={
            summary?.avg_first_token_ms == null
              ? '—'
              : `${Math.round(summary.avg_first_token_ms)} ms`
          }
          icon={Zap}
        />
      </div>

      <div className="mt-4">
        <Suspense
          fallback={
            <div className="grid gap-4 lg:grid-cols-2">
              {Array.from({ length: 4 }).map((_, index) => (
                <Skeleton key={index} className="h-[340px] rounded-xl" />
              ))}
            </div>
          }
        >
          <DashboardCharts
            points={timeseriesQuery.data?.points ?? []}
            models={modelsQuery.data?.items ?? []}
            channels={channelsQuery.data?.items ?? []}
          />
        </Suspense>
      </div>

      <div className="mt-4 grid gap-4 xl:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)]">
        <Card>
          <CardHeader>
            <CardTitle>最近错误</CardTitle>
          </CardHeader>
          <CardContent>
            {errorsQuery.isLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 3 }).map((_, index) => (
                  <Skeleton key={index} className="h-10 w-full" />
                ))}
              </div>
            ) : errors.length === 0 ? (
              <EmptyState
                title="当前范围没有错误请求"
                description="错误请求会显示在这里，便于快速定位上游问题。"
              />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>时间</TableHead>
                    <TableHead>模型</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead>错误</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {errors.map((entry) => (
                    <TableRow key={entry.id}>
                      <TableCell className="text-xs text-muted-foreground">
                        {formatDateTime(entry.created_at, 'MM-DD HH:mm:ss')}
                      </TableCell>
                      <TableCell className="max-w-[180px] truncate font-mono text-xs">
                        {entry.model}
                      </TableCell>
                      <TableCell>
                        <StatusCodeBadge code={entry.status_code} />
                      </TableCell>
                      <TableCell
                        className="max-w-[260px] truncate text-xs text-muted-foreground"
                        title={entry.error_message ?? undefined}
                      >
                        {entry.error_message
                          ? `${entry.error_type ?? 'error'}: ${entry.error_message}`
                          : '-'}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>系统信息</CardTitle>
          </CardHeader>
          <CardContent>
            {systemQuery.isLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 5 }).map((_, index) => (
                  <Skeleton key={index} className="h-8 w-full" />
                ))}
              </div>
            ) : (
              <dl className="grid grid-cols-2 gap-x-4 gap-y-4">
                <InfoItem label="名称" value={systemQuery.data?.name ?? '-'} />
                <InfoItem
                  label="版本"
                  value={systemQuery.data?.version ?? '-'}
                />
                <InfoItem
                  label="运行时长"
                  value={
                    systemQuery.data
                      ? formatUptime(systemQuery.data.uptime_seconds)
                      : '-'
                  }
                />
                <InfoItem
                  label="数据库"
                  value={
                    <Badge
                      variant="outline"
                      className={
                        systemQuery.data?.database === 'ok'
                          ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                          : 'border-destructive/30 bg-destructive/10 text-destructive'
                      }
                    >
                      {systemQuery.data?.database ?? '-'}
                    </Badge>
                  }
                />
                <div className="col-span-2">
                  <InfoItem
                    label="数据目录"
                    value={
                      <span className="font-mono text-xs">
                        {systemQuery.data?.data_dir ?? '-'}
                      </span>
                    }
                  />
                </div>
                <InfoItem
                  label="活跃渠道"
                  value={summary?.active_channels ?? '-'}
                />
                <InfoItem
                  label="活跃 Key"
                  value={summary?.active_keys ?? '-'}
                />
              </dl>
            )}
          </CardContent>
        </Card>
      </div>
    </>
  );
}
