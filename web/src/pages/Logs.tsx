import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { RotateCcw, Search } from 'lucide-react';
import { useMemo, useState, type ReactNode } from 'react';
import { api } from '@/api/client';
import type {
  ApiKey,
  Channel,
  LogDetail,
  LogEntry,
  LogList,
} from '@/api/types';
import {
  ProtocolBadge,
  SourceBadge,
  StatusCodeBadge,
  StreamBadge,
} from '@/components/common/badges';
import { CodeBlock } from '@/components/common/code-block';
import { EmptyState } from '@/components/common/empty-state';
import { PageHeader } from '@/components/common/page-header';
import { Pagination } from '@/components/common/pagination';
import { useDebouncedValue } from '@/hooks/useDebouncedValue';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet';
import { Skeleton } from '@/components/ui/skeleton';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import {
  formatDateTime,
  formatLatency,
  formatNumber,
  formatTokens,
  formatTokensPerSecond,
} from '@/lib/format';
import { useLiveConnected } from '@/hooks/useLiveMetrics';
import {
  useLivePollingInterval,
  useLiveRefresh,
} from '@/hooks/useLiveRefresh';

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
  channelId: number | null;
  apiKeyId: number | null;
  status: string;
  protocol: string;
  source: string;
  stream: 'all' | 'true' | 'false';
}

const INITIAL_FILTERS: Filters = {
  range: '24h',
  model: '',
  requestId: '',
  channelId: null,
  apiKeyId: null,
  status: 'all',
  protocol: 'all',
  source: 'all',
  stream: 'all',
};

function DetailRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="min-w-0 space-y-0.5">
      <dt className="text-xs text-muted-foreground">{label}</dt>
      <dd
        className={
          mono
            ? 'truncate font-mono text-xs'
            : 'truncate text-sm text-foreground'
        }
      >
        {value}
      </dd>
    </div>
  );
}

export default function Logs() {
  const [filters, setFilters] = useState<Filters>(INITIAL_FILTERS);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(20);
  const [selected, setSelected] = useState<LogEntry | null>(null);
  // The COUNT(*) scan is skipped for pages after the first; keep the total
  // from the first page so pagination stays stable while browsing.
  const [knownTotal, setKnownTotal] = useState(0);
  const connected = useLiveConnected();
  const refetchInterval = useLivePollingInterval();
  // Typing must not fire one /api/logs request per keystroke.
  const modelTerm = useDebouncedValue(filters.model, 300);
  const requestIdTerm = useDebouncedValue(filters.requestId, 300);

  // New usage rows arrive as soon as the writer commits them; the minimum
  // interval keeps a busy gateway from triggering one list query per request.
  useLiveRefresh([['logs']], 1500);

  function update<K extends keyof Filters>(key: K, value: Filters[K]) {
    setFilters((previous) => ({ ...previous, [key]: value }));
    setPage(1);
    setKnownTotal(0);
  }

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
    if (modelTerm.trim()) params.set('model', modelTerm.trim());
    if (requestIdTerm.trim()) params.set('request_id', requestIdTerm.trim());
    if (filters.channelId != null)
      params.set('channel_id', String(filters.channelId));
    if (filters.apiKeyId != null)
      params.set('api_key_id', String(filters.apiKeyId));
    if (filters.status !== 'all') params.set('status', filters.status);
    if (filters.protocol !== 'all') params.set('protocol', filters.protocol);
    if (filters.source !== 'all') params.set('source', filters.source);
    if (filters.stream !== 'all') params.set('stream', filters.stream);
    params.set('include_total', page === 1 ? 'true' : 'false');
    return `/api/logs?${params.toString()}`;
  }, [filters, modelTerm, requestIdTerm, page, pageSize]);

  const logsQuery = useQuery({
    queryKey: ['logs', path],
    queryFn: () => api.get<LogList>(path),
    refetchInterval,
    // Keep the current rows visible while a new filter value loads.
    placeholderData: keepPreviousData,
  });

  const total = logsQuery.data?.total;

  const detailQuery = useQuery({
    queryKey: ['log-detail', selected?.request_id],
    queryFn: () => api.get<LogDetail>(`/api/logs/${selected?.request_id}`),
    enabled: selected != null,
  });

  const logs = logsQuery.data?.items ?? [];

  return (
    <>
      <PageHeader
        title="日志"
        description="查看每次请求的协议转换、路由结果、Token 用量与耗时。"
        actions={
          <Badge
            variant="outline"
            className={
              connected
                ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                : 'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300'
            }
          >
            <span
              className={
                connected
                  ? 'mr-1.5 size-1.5 animate-pulse rounded-full bg-current'
                  : 'mr-1.5 size-1.5 rounded-full bg-current'
              }
            />
            {connected ? '实时更新中' : '连接中断，轮询中'}
          </Badge>
        }
      />

      <Card className="mb-4">
        <CardContent className="grid grid-cols-2 gap-2 md:grid-cols-3 xl:grid-cols-6">
          <Select
            items={RANGE_OPTIONS}
            value={filters.range}
            onValueChange={(value) => {
              if (value) {
                update('range', value);
              }
            }}
          >
            <SelectTrigger className="w-full">
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

          <Input
            placeholder="模型"
            value={filters.model}
            onChange={(event) => update('model', event.target.value)}
          />
          <Input
            placeholder="请求 ID"
            value={filters.requestId}
            onChange={(event) => update('requestId', event.target.value)}
          />

          <Select
            items={[
              { value: 'all', label: '全部渠道' },
              ...(channelsQuery.data ?? []).map((channel) => ({
                value: String(channel.id),
                label: channel.name,
              })),
            ]}
            value={filters.channelId == null ? 'all' : String(filters.channelId)}
            onValueChange={(value) => {
              if (value != null) {
                update('channelId', value === 'all' ? null : Number(value));
              }
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue placeholder="渠道" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部渠道</SelectItem>
              {(channelsQuery.data ?? []).map((channel) => (
                <SelectItem key={channel.id} value={String(channel.id)}>
                  {channel.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select
            items={[
              { value: 'all', label: '全部 Key' },
              ...(keysQuery.data ?? []).map((key) => ({
                value: String(key.id),
                label: key.name,
              })),
            ]}
            value={filters.apiKeyId == null ? 'all' : String(filters.apiKeyId)}
            onValueChange={(value) => {
              if (value != null) {
                update('apiKeyId', value === 'all' ? null : Number(value));
              }
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue placeholder="API Key" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="all">全部 Key</SelectItem>
              {(keysQuery.data ?? []).map((key) => (
                <SelectItem key={key.id} value={String(key.id)}>
                  {key.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select
            items={STATUS_OPTIONS}
            value={filters.status}
            onValueChange={(value) => {
              if (value) {
                update('status', value);
              }
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select
            items={PROTOCOL_OPTIONS}
            value={filters.protocol}
            onValueChange={(value) => {
              if (value) {
                update('protocol', value);
              }
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {PROTOCOL_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select
            items={SOURCE_OPTIONS}
            value={filters.source}
            onValueChange={(value) => {
              if (value) {
                update('source', value);
              }
            }}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {SOURCE_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Select
            items={STREAM_OPTIONS}
            value={filters.stream}
            onValueChange={(value) =>
              update('stream', value as Filters['stream'])
            }
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {STREAM_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>

          <Button
            variant="outline"
            onClick={() => {
              setFilters(INITIAL_FILTERS);
              setPage(1);
              setKnownTotal(0);
            }}
          >
            <RotateCcw />
            重置
          </Button>
        </CardContent>
      </Card>

      {logsQuery.isError ? (
        <Alert variant="destructive">
          <AlertTitle>加载日志失败</AlertTitle>
          <AlertDescription>
            {logsQuery.error instanceof Error
              ? logsQuery.error.message
              : String(logsQuery.error)}
          </AlertDescription>
        </Alert>
      ) : (
        <Card className="overflow-hidden">
          <CardContent className="p-0">
            {logsQuery.isLoading ? (
              <div className="space-y-2 p-4">
                {Array.from({ length: 8 }).map((_, index) => (
                  <Skeleton key={index} className="h-10 w-full" />
                ))}
              </div>
            ) : logs.length === 0 ? (
              <div className="p-6">
                <EmptyState
                  title="没有匹配的日志"
                  description="尝试放宽筛选条件或切换时间范围。"
                  icon={<Search className="size-5" />}
                />
              </div>
            ) : (
              <div className="overflow-x-auto">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="whitespace-nowrap">时间</TableHead>
                      <TableHead className="hidden 2xl:table-cell">
                        请求 ID
                      </TableHead>
                      <TableHead>模型</TableHead>
                      <TableHead className="hidden md:table-cell">协议</TableHead>
                      <TableHead className="hidden lg:table-cell">来源</TableHead>
                      <TableHead className="hidden lg:table-cell">渠道</TableHead>
                      <TableHead className="hidden 2xl:table-cell">
                        API Key
                      </TableHead>
                      <TableHead>状态</TableHead>
                      <TableHead className="hidden 2xl:table-cell">模式</TableHead>
                      <TableHead className="hidden sm:table-cell">Tokens</TableHead>
                      <TableHead>延迟</TableHead>
                      <TableHead className="hidden 2xl:table-cell">速度</TableHead>
                      <TableHead className="hidden 2xl:table-cell">尝试</TableHead>
                      <TableHead>错误</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {logs.map((entry) => (
                      <TableRow
                        key={entry.id}
                        className="cursor-pointer"
                        onClick={() => setSelected(entry)}
                      >
                        <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                          {formatDateTime(entry.created_at)}
                        </TableCell>
                        <TableCell className="hidden 2xl:table-cell">
                          <span
                            className="font-mono text-xs text-muted-foreground"
                            title={entry.request_id}
                          >
                            {entry.request_id.slice(0, 12)}…
                          </span>
                        </TableCell>
                        <TableCell className="max-w-[160px] truncate font-mono text-xs">
                          {entry.model}
                        </TableCell>
                        <TableCell className="hidden md:table-cell">
                          <div className="flex items-center gap-1 whitespace-nowrap">
                            <ProtocolBadge protocol={entry.inbound_protocol} />
                            <span className="text-xs text-muted-foreground">
                              →
                            </span>
                            <ProtocolBadge protocol={entry.upstream_protocol} />
                          </div>
                        </TableCell>
                        <TableCell className="hidden lg:table-cell">
                          <SourceBadge source={entry.source} />
                        </TableCell>
                        <TableCell className="hidden max-w-[140px] truncate text-xs lg:table-cell">
                          {entry.channel_name ??
                            (entry.channel_id ? `#${entry.channel_id}` : '-')}
                        </TableCell>
                        <TableCell className="hidden max-w-[130px] truncate text-xs 2xl:table-cell">
                          {entry.api_key_name ??
                            (entry.api_key_id ? `#${entry.api_key_id}` : '-')}
                        </TableCell>
                        <TableCell>
                          <StatusCodeBadge code={entry.status_code} />
                        </TableCell>
                        <TableCell className="hidden 2xl:table-cell">
                          <StreamBadge stream={entry.stream} />
                        </TableCell>
                        <TableCell
                          className="hidden whitespace-nowrap text-xs tabular-nums sm:table-cell"
                          title={`输入 ${formatNumber(entry.prompt_tokens)} / 输出 ${formatNumber(
                            entry.completion_tokens,
                          )}`}
                        >
                          {formatTokens(entry.total_tokens)}
                        </TableCell>
                        <TableCell
                          className="whitespace-nowrap text-xs tabular-nums"
                          title={`首 token ${formatLatency(entry.first_token_ms)}`}
                        >
                          {formatLatency(entry.latency_ms)}
                        </TableCell>
                        <TableCell
                          className="hidden whitespace-nowrap text-xs tabular-nums 2xl:table-cell"
                          title="输出 tokens ÷ 生成耗时；流式已扣除首 token 等待"
                        >
                          {formatTokensPerSecond(entry.tokens_per_second)}
                        </TableCell>
                        <TableCell className="hidden text-xs tabular-nums 2xl:table-cell">
                          {entry.attempts}
                        </TableCell>
                        <TableCell className="max-w-[200px] truncate text-xs">
                          {entry.error_message ? (
                            <span
                              className="text-destructive"
                              title={entry.error_message}
                            >
                              {entry.error_type ?? 'error'}:{' '}
                              {entry.error_message}
                            </span>
                          ) : (
                            '-'
                          )}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              </div>
            )}

            <div className="px-4 pb-4">
              <Pagination
                page={page}
                pageSize={pageSize}
                total={total ?? knownTotal}
                onPageChange={(next) => {
                  // Capture the first page's count before deep pages stop
                  // requesting it.
                  if (total != null) {
                    setKnownTotal(total);
                  }
                  setPage(next);
                }}
                onPageSizeChange={(size) => {
                  setPageSize(size);
                  setPage(1);
                }}
              />
            </div>
          </CardContent>
        </Card>
      )}

      <Sheet
        open={selected != null}
        onOpenChange={(next) => {
          if (!next) {
            setSelected(null);
          }
        }}
      >
        <SheetContent
          side="right"
          className="gap-0 p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-[760px]"
        >
          <SheetHeader className="border-b p-4">
            <SheetTitle>请求详情</SheetTitle>
          </SheetHeader>

          <div className="flex-1 space-y-6 overflow-y-auto p-4">
            {detailQuery.isLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 8 }).map((_, index) => (
                  <Skeleton key={index} className="h-9 w-full" />
                ))}
              </div>
            ) : detailQuery.isError ? (
              <Alert variant="destructive">
                <AlertTitle>加载详情失败</AlertTitle>
                <AlertDescription>
                  {detailQuery.error instanceof Error
                    ? detailQuery.error.message
                    : String(detailQuery.error)}
                </AlertDescription>
              </Alert>
            ) : detailQuery.data ? (
              <>
                <dl className="grid grid-cols-2 gap-x-4 gap-y-4 sm:grid-cols-3">
                  <div className="col-span-2 space-y-0.5 sm:col-span-3">
                    <dt className="text-xs text-muted-foreground">请求 ID</dt>
                    <dd className="break-all font-mono text-xs">
                      {detailQuery.data.request_id}
                    </dd>
                  </div>
                  <DetailRow
                    label="时间"
                    value={formatDateTime(detailQuery.data.created_at)}
                  />
                  <DetailRow
                    label="来源"
                    value={<SourceBadge source={detailQuery.data.source} />}
                  />
                  <DetailRow
                    label="状态码"
                    value={<StatusCodeBadge code={detailQuery.data.status_code} />}
                  />
                  <DetailRow
                    label="入站协议"
                    value={
                      <ProtocolBadge
                        protocol={detailQuery.data.inbound_protocol}
                      />
                    }
                  />
                  <DetailRow
                    label="上游协议"
                    value={
                      <ProtocolBadge
                        protocol={detailQuery.data.upstream_protocol}
                      />
                    }
                  />
                  <DetailRow
                    label="流式"
                    value={<StreamBadge stream={detailQuery.data.stream} />}
                  />
                  <DetailRow
                    label="客户端模型"
                    value={detailQuery.data.model}
                    mono
                  />
                  <DetailRow
                    label="上游模型"
                    value={detailQuery.data.upstream_model ?? '-'}
                    mono
                  />
                  <DetailRow
                    label="尝试次数"
                    value={detailQuery.data.attempts}
                  />
                  <DetailRow
                    label="渠道"
                    value={
                      detailQuery.data.channel_name ??
                      (detailQuery.data.channel_id
                        ? `#${detailQuery.data.channel_id}`
                        : '-')
                    }
                  />
                  <DetailRow
                    label="API Key"
                    value={
                      detailQuery.data.api_key_name ??
                      (detailQuery.data.api_key_id
                        ? `#${detailQuery.data.api_key_id}`
                        : '-')
                    }
                  />
                  <DetailRow label="延迟" value={formatLatency(detailQuery.data.latency_ms)} />
                  <DetailRow
                    label="首 token"
                    value={formatLatency(detailQuery.data.first_token_ms)}
                  />
                  <DetailRow
                    label="生成速度"
                    value={formatTokensPerSecond(
                      detailQuery.data.tokens_per_second,
                    )}
                  />
                  <DetailRow
                    label="输入 Tokens"
                    value={formatNumber(detailQuery.data.prompt_tokens)}
                  />
                  <DetailRow
                    label="输出 Tokens"
                    value={formatNumber(detailQuery.data.completion_tokens)}
                  />
                  <DetailRow
                    label="总 Tokens"
                    value={formatNumber(detailQuery.data.total_tokens)}
                  />
                  <DetailRow
                    label="缓存读取"
                    value={formatNumber(detailQuery.data.cached_tokens)}
                  />
                  <DetailRow
                    label="缓存写入"
                    value={formatNumber(detailQuery.data.cache_creation_tokens)}
                  />
                  <DetailRow
                    label="推理 Tokens"
                    value={formatNumber(detailQuery.data.reasoning_tokens)}
                  />
                </dl>

                {detailQuery.data.error_message ? (
                  <Alert variant="destructive">
                    <AlertTitle>
                      {detailQuery.data.error_type ?? 'error'}
                    </AlertTitle>
                    <AlertDescription className="break-words">
                      {detailQuery.data.error_message}
                    </AlertDescription>
                  </Alert>
                ) : null}

                <div className="space-y-2">
                  <h3 className="text-sm font-medium">请求体</h3>
                  {detailQuery.data.request_body ? (
                    <CodeBlock>{detailQuery.data.request_body}</CodeBlock>
                  ) : (
                    <p className="rounded-lg border bg-muted/40 p-3 text-xs text-muted-foreground">
                      未记录请求体
                    </p>
                  )}
                </div>

                <div className="space-y-2">
                  <h3 className="text-sm font-medium">响应体</h3>
                  {detailQuery.data.response_body ? (
                    <CodeBlock>{detailQuery.data.response_body}</CodeBlock>
                  ) : (
                    <p className="rounded-lg border bg-muted/40 p-3 text-xs text-muted-foreground">
                      未记录响应体
                    </p>
                  )}
                </div>

                {!detailQuery.data.request_body &&
                !detailQuery.data.response_body ? (
                  <Alert>
                    <AlertTitle>未记录请求/响应体</AlertTitle>
                    <AlertDescription>
                      可在「设置 → 记录请求/响应体」中开启；开启后单条最多保存
                      64KB。
                    </AlertDescription>
                  </Alert>
                ) : null}
              </>
            ) : null}
          </div>
        </SheetContent>
      </Sheet>
    </>
  );
}
