import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Download,
  Loader2,
  Pencil,
  Plus,
  PlugZap,
  Trash2,
} from 'lucide-react';
import { useMemo, useState } from 'react';
import { toast } from 'sonner';
import { api } from '@/api/client';
import type {
  Channel,
  ChannelKind,
  ChannelTestResult,
  ModelFetchResult,
} from '@/api/types';
import { EnabledBadge, ProtocolBadge } from '@/components/common/badges';
import { ConfirmDialog } from '@/components/common/confirm-dialog';
import { EmptyState } from '@/components/common/empty-state';
import { Field } from '@/components/common/field';
import { KeyValueEditor } from '@/components/common/key-value-editor';
import { PageHeader } from '@/components/common/page-header';
import { TagInput } from '@/components/common/tag-input';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
} from '@/components/ui/card';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
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
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet';
import { Skeleton } from '@/components/ui/skeleton';
import { Switch } from '@/components/ui/switch';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table';
import { useLiveMetrics } from '@/hooks/useLiveMetrics';

interface MappingRow {
  key: string;
  value: string;
}

interface ChannelDraft {
  name: string;
  kinds: ChannelKind[];
  fallbackKind: ChannelKind;
  baseUrl: string;
  apiKey: string;
  authHeader: 'bearer' | 'x-api-key';
  models: string[];
  modelMapping: MappingRow[];
  extraHeaders: MappingRow[];
  priority: string;
  weight: string;
  timeoutMs: string;
  proxyUrl: string;
  enabled: boolean;
}

const KIND_LABELS: Record<ChannelKind, string> = {
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  anthropic: 'Anthropic',
};

const KIND_OPTIONS = (Object.keys(KIND_LABELS) as ChannelKind[]).map((value) => ({
  value,
  label: KIND_LABELS[value],
}));

const AUTH_HEADER_OPTIONS = [
  { value: 'bearer', label: 'Authorization: Bearer' },
  { value: 'x-api-key', label: 'x-api-key' },
];

function emptyDraft(): ChannelDraft {
  return {
    name: '',
    kinds: ['openai_chat'],
    fallbackKind: 'openai_chat',
    baseUrl: '',
    apiKey: '',
    authHeader: 'bearer',
    models: [],
    modelMapping: [],
    extraHeaders: [],
    priority: '0',
    weight: '1',
    timeoutMs: '',
    proxyUrl: '',
    enabled: true,
  };
}

function draftFromChannel(channel: Channel): ChannelDraft {
  return {
    name: channel.name,
    kinds: channel.kinds,
    fallbackKind: channel.fallback_kind,
    baseUrl: channel.base_url,
    apiKey: '',
    authHeader: channel.auth_header,
    models: channel.models,
    modelMapping: Object.entries(channel.model_mapping).map(([key, value]) => ({
      key,
      value,
    })),
    extraHeaders: Object.entries(channel.extra_headers).map(([key, value]) => ({
      key,
      value,
    })),
    priority: String(channel.priority),
    weight: String(channel.weight),
    timeoutMs: channel.timeout_ms == null ? '' : String(channel.timeout_ms),
    proxyUrl: channel.proxy_url ?? '',
    enabled: channel.enabled,
  };
}

function toNumber(value: string, fallback: number): number {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function toPayload(draft: ChannelDraft, editing: boolean) {
  return {
    name: draft.name.trim(),
    kinds: draft.kinds,
    fallback_kind: draft.fallbackKind,
    base_url: draft.baseUrl.trim(),
    api_key: draft.apiKey.trim() || (editing ? undefined : ''),
    auth_header: draft.authHeader,
    models: draft.models,
    model_mapping: Object.fromEntries(
      draft.modelMapping
        .filter((row) => row.key.trim())
        .map((row) => [row.key.trim(), row.value.trim()]),
    ),
    extra_headers: Object.fromEntries(
      draft.extraHeaders
        .filter((row) => row.key.trim())
        .map((row) => [row.key.trim(), row.value.trim()]),
    ),
    priority: toNumber(draft.priority, 0),
    weight: toNumber(draft.weight, 1),
    timeout_ms: draft.timeoutMs.trim()
      ? Math.max(1, toNumber(draft.timeoutMs, 1))
      : null,
    proxy_url: draft.proxyUrl.trim() || null,
    enabled: draft.enabled,
  };
}

function validateDraft(draft: ChannelDraft, editing: boolean) {
  const errors: Record<string, string> = {};
  if (!draft.name.trim()) {
    errors.name = '请输入渠道名称';
  }
  if (draft.kinds.length === 0) {
    errors.kinds = '至少选择一种协议';
  }
  if (!draft.kinds.includes(draft.fallbackKind)) {
    errors.fallbackKind = '转换目标必须是已支持的协议';
  }
  if (!draft.baseUrl.trim()) {
    errors.baseUrl = '请输入 Base URL';
  }
  if (!editing && !draft.apiKey.trim()) {
    errors.apiKey = '请输入上游 API Key';
  }
  if (draft.models.length === 0) {
    errors.models = '至少填写一个模型';
  }
  return errors;
}

export default function Channels() {
  const queryClient = useQueryClient();
  const live = useLiveMetrics();
  const liveRates = useMemo(
    () => new Map(live.channels.map((item) => [item.channel_id, item.tok_s])),
    [live],
  );

  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [draft, setDraft] = useState<ChannelDraft>(emptyDraft);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [deleteTarget, setDeleteTarget] = useState<Channel | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [modelDialogOpen, setModelDialogOpen] = useState(false);

  const channelsQuery = useQuery({
    queryKey: ['channels'],
    queryFn: () => api.get<Channel[]>('/api/channels'),
  });

  function update(patch: Partial<ChannelDraft>) {
    setDraft((current) => {
      const next = { ...current, ...patch };
      if (patch.kinds && !next.kinds.includes(next.fallbackKind)) {
        next.fallbackKind = next.kinds[0] ?? 'openai_chat';
      }
      return next;
    });
    setErrors((current) => {
      const next = { ...current };
      for (const key of Object.keys(patch)) {
        delete next[key];
      }
      return next;
    });
  }

  const saveMutation = useMutation({
    mutationFn: ({ id, payload }: { id?: number; payload: unknown }) =>
      id
        ? api.put<Channel>(`/api/channels/${id}`, payload)
        : api.post<Channel>('/api/channels', payload),
    onSuccess: () => {
      toast.success('渠道已保存');
      setOpen(false);
      setEditing(null);
      queryClient.invalidateQueries({ queryKey: ['channels'] });
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/api/channels/${id}`),
    onSuccess: () => {
      toast.success('渠道已删除');
      setDeleteTarget(null);
      queryClient.invalidateQueries({ queryKey: ['channels'] });
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  function reportTest(result: ChannelTestResult) {
    if (result.ok) {
      toast.success(`连通成功，${result.latency_ms} ms`);
    } else {
      toast.error(
        `测试失败（${result.status || '网络错误'}）：${result.message}`,
      );
    }
  }

  const testMutation = useMutation({
    mutationFn: (id: number) =>
      api.post<ChannelTestResult>(`/api/channels/${id}/test`),
    onSuccess: reportTest,
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  const probeTestMutation = useMutation({
    mutationFn: (payload: unknown) =>
      api.post<ChannelTestResult>('/api/channels/test', payload),
    onSuccess: reportTest,
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  const probeFetchMutation = useMutation({
    mutationFn: (payload: unknown) =>
      api.post<ModelFetchResult>('/api/channels/fetch-models', payload),
    onSuccess: (result) => {
      if (result.models.length === 0) {
        toast.warning(result.message || '未拉取到模型');
        return;
      }
      setFetchedModels(result.models);
      setSelectedModels(result.models);
      setModelDialogOpen(true);
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  function probe(action: 'test' | 'fetch-models') {
    const nextErrors = validateDraft(draft, editing != null);
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      toast.error('请先补全表单必填项');
      return;
    }

    const payload = {
      ...toPayload(draft, editing != null),
      id: editing?.id,
    };
    if (action === 'test') {
      probeTestMutation.mutate(payload);
    } else {
      probeFetchMutation.mutate(payload);
    }
  }

  function openCreate() {
    setEditing(null);
    setDraft(emptyDraft());
    setErrors({});
    setOpen(true);
  }

  function openEdit(channel: Channel) {
    setEditing(channel);
    setDraft(draftFromChannel(channel));
    setErrors({});
    setOpen(true);
  }

  function submit() {
    const nextErrors = validateDraft(draft, editing != null);
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    saveMutation.mutate({
      id: editing?.id,
      payload: toPayload(draft, editing != null),
    });
  }

  function toggleModel(model: string) {
    setSelectedModels((current) =>
      current.includes(model)
        ? current.filter((item) => item !== model)
        : [...current, model],
    );
  }

  const channels = channelsQuery.data ?? [];
  const availableKinds = KIND_OPTIONS.filter((option) =>
    draft.kinds.includes(option.value),
  );

  return (
    <>
      <PageHeader
        title="渠道"
        description="同一个模型可以配置多个渠道，请求会按优先级和权重路由。"
        actions={
          <>
            <Badge
              variant="outline"
              className={
                live.global_tok_s > 0
                  ? 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                  : 'text-muted-foreground'
              }
            >
              全站实时 {live.global_tok_s.toFixed(1)} tok/s
            </Badge>
            <Button onClick={openCreate}>
              <Plus />
              新建渠道
            </Button>
          </>
        }
      />

      <Card className="overflow-hidden">
        <CardContent className="p-0">
          {channelsQuery.isLoading ? (
            <div className="space-y-2 p-4">
              {Array.from({ length: 4 }).map((_, index) => (
                <Skeleton key={index} className="h-12 w-full" />
              ))}
            </div>
          ) : channels.length === 0 ? (
            <div className="p-6">
              <EmptyState
                title="还没有配置渠道"
                description="新建一个上游渠道后，网关即可转发请求。"
                action={
                  <Button onClick={openCreate}>
                    <Plus />
                    新建渠道
                  </Button>
                }
              />
            </div>
          ) : (
            <div className="overflow-x-auto">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>名称</TableHead>
                    <TableHead className="hidden sm:table-cell">协议</TableHead>
                    <TableHead className="hidden lg:table-cell">
                      Base URL
                    </TableHead>
                    <TableHead className="hidden md:table-cell">模型</TableHead>
                    <TableHead className="hidden xl:table-cell">
                      优先级 / 权重
                    </TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead className="hidden lg:table-cell">
                      实时 tok/s
                    </TableHead>
                    <TableHead className="text-right">操作</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {channels.map((channel) => {
                    const rate = liveRates.get(channel.id) ?? 0;
                    return (
                      <TableRow key={channel.id}>
                        <TableCell>
                          <div className="space-y-0.5">
                            <p className="font-medium">{channel.name}</p>
                            <p className="font-mono text-xs text-muted-foreground">
                              {channel.api_key_masked}
                            </p>
                          </div>
                        </TableCell>
                        <TableCell className="hidden sm:table-cell">
                          <div className="flex flex-wrap gap-1">
                            {(channel.kinds ?? []).map((kind) => (
                              <ProtocolBadge key={kind} protocol={kind} />
                            ))}
                          </div>
                        </TableCell>
                        <TableCell className="hidden max-w-[220px] truncate font-mono text-xs text-muted-foreground lg:table-cell">
                          {channel.base_url}
                        </TableCell>
                        <TableCell className="hidden md:table-cell">
                          <div className="flex items-center gap-1.5">
                            <span className="text-sm">
                              {channel.models.length} 个
                            </span>
                            {channel.models.includes('*') ? (
                              <Badge variant="secondary" className="text-xs">
                                通配
                              </Badge>
                            ) : null}
                          </div>
                        </TableCell>
                        <TableCell className="hidden tabular-nums text-sm xl:table-cell">
                          {channel.priority} / {channel.weight}
                        </TableCell>
                        <TableCell>
                          <div className="flex flex-wrap items-center gap-1">
                            <EnabledBadge
                              enabled={channel.enabled}
                              disabledUntil={channel.disabled_until}
                            />
                            {channel.consecutive_failures > 0 ? (
                              <Badge
                                variant="outline"
                                className="border-destructive/30 bg-destructive/10 text-xs text-destructive"
                              >
                                连续失败 {channel.consecutive_failures}
                              </Badge>
                            ) : null}
                          </div>
                          {channel.last_error ? (
                            <p
                              className="mt-1 max-w-[220px] truncate text-xs text-muted-foreground"
                              title={channel.last_error}
                            >
                              {channel.last_error}
                            </p>
                          ) : null}
                        </TableCell>
                        <TableCell className="hidden lg:table-cell">
                          <Badge
                            variant="outline"
                            className={
                              rate > 0
                                ? 'border-emerald-500/30 bg-emerald-500/10 font-mono text-emerald-700 dark:text-emerald-300'
                                : 'font-mono text-muted-foreground'
                            }
                          >
                            {rate.toFixed(1)} tok/s
                          </Badge>
                        </TableCell>
                        <TableCell>
                          <div className="flex justify-end gap-1">
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => testMutation.mutate(channel.id)}
                              disabled={testMutation.isPending}
                            >
                              {testMutation.isPending &&
                              testMutation.variables === channel.id ? (
                                <Loader2 className="animate-spin" />
                              ) : (
                                <PlugZap />
                              )}
                              <span className="hidden sm:inline">测试</span>
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => openEdit(channel)}
                            >
                              <Pencil />
                              <span className="hidden sm:inline">编辑</span>
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-destructive hover:text-destructive"
                              onClick={() => setDeleteTarget(channel)}
                            >
                              <Trash2 />
                              <span className="hidden sm:inline">删除</span>
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>

      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent
          side="right"
          className="gap-0 p-0 data-[side=right]:w-full data-[side=right]:sm:max-w-[600px]"
        >
          <SheetHeader className="border-b p-4">
            <SheetTitle>{editing ? '编辑渠道' : '新建渠道'}</SheetTitle>
          </SheetHeader>

          <div className="flex flex-wrap items-center gap-2 border-b bg-muted/40 px-4 py-3">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => probe('test')}
              disabled={probeTestMutation.isPending}
            >
              {probeTestMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <PlugZap />
              )}
              测试连通性
            </Button>
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => probe('fetch-models')}
              disabled={probeFetchMutation.isPending}
            >
              {probeFetchMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <Download />
              )}
              拉取模型
            </Button>
            <span className="text-xs text-muted-foreground">
              使用当前表单配置，无需先保存
            </span>
          </div>

          <div className="flex-1 space-y-4 overflow-y-auto p-4">
            <Field label="名称" htmlFor="channel-name" error={errors.name}>
              <Input
                id="channel-name"
                value={draft.name}
                onChange={(event) => update({ name: event.target.value })}
                placeholder="例如：OpenAI 主账号"
              />
            </Field>

            <Field
              label="支持的协议"
              error={errors.kinds}
              hint="入站协议在列表中时原样转发；不在列表中时转换到下方「转换目标」。"
            >
              <div className="flex flex-wrap gap-2">
                {KIND_OPTIONS.map((option) => {
                  const active = draft.kinds.includes(option.value);
                  return (
                    <Button
                      key={option.value}
                      type="button"
                      variant={active ? 'default' : 'outline'}
                      size="sm"
                      onClick={() =>
                        update({
                          kinds: active
                            ? draft.kinds.filter(
                                (kind) => kind !== option.value,
                              )
                            : [...draft.kinds, option.value],
                        })
                      }
                    >
                      {option.label}
                    </Button>
                  );
                })}
              </div>
            </Field>

            <Field
              label="转换目标"
              error={errors.fallbackKind}
              hint="只有入站协议不被该渠道支持时才会使用。"
            >
              <Select
                items={availableKinds}
                value={draft.fallbackKind}
                onValueChange={(value) =>
                  update({ fallbackKind: value as ChannelKind })
                }
                disabled={availableKinds.length === 0}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="选择转换目标" />
                </SelectTrigger>
                <SelectContent>
                  {availableKinds.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>

            <Field label="Base URL" htmlFor="channel-url" error={errors.baseUrl}>
              <Input
                id="channel-url"
                value={draft.baseUrl}
                onChange={(event) => update({ baseUrl: event.target.value })}
                placeholder="https://api.openai.com/v1"
                className="font-mono"
              />
            </Field>

            <Field
              label="上游 API Key"
              htmlFor="channel-key"
              error={errors.apiKey}
              hint={editing ? '留空表示保持原值。' : undefined}
            >
              <Input
                id="channel-key"
                type="password"
                autoComplete="new-password"
                value={draft.apiKey}
                onChange={(event) => update({ apiKey: event.target.value })}
                placeholder={editing ? '留空保持不变' : 'sk-...'}
                className="font-mono"
              />
            </Field>

            <Field label="鉴权头">
              <Select
                items={AUTH_HEADER_OPTIONS}
                value={draft.authHeader}
                onValueChange={(value) =>
                  update({ authHeader: value as 'bearer' | 'x-api-key' })
                }
              >
                <SelectTrigger className="w-full">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="bearer">
                    Authorization: Bearer
                  </SelectItem>
                  <SelectItem value="x-api-key">x-api-key</SelectItem>
                </SelectContent>
              </Select>
            </Field>

            <Field
              label="模型"
              error={errors.models}
              hint="输入模型名后回车；* 表示接受任意模型。"
            >
              <TagInput
                value={draft.models}
                onChange={(models) => update({ models })}
                placeholder="gpt-4o-mini"
              />
            </Field>

            <Field
              label="模型映射"
              hint="客户端模型 → 上游模型；未命中的模型原样转发。"
            >
              <KeyValueEditor
                value={draft.modelMapping}
                onChange={(modelMapping) => update({ modelMapping })}
                keyPlaceholder="客户端模型"
                valuePlaceholder="上游模型"
              />
            </Field>

            <Field label="自定义请求头">
              <KeyValueEditor
                value={draft.extraHeaders}
                onChange={(extraHeaders) => update({ extraHeaders })}
                keyPlaceholder="Header 名称"
                valuePlaceholder="Header 值"
              />
            </Field>

            <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
              <Field label="优先级" htmlFor="channel-priority">
                <Input
                  id="channel-priority"
                  type="number"
                  min={-1000}
                  max={1000}
                  value={draft.priority}
                  onChange={(event) =>
                    update({ priority: event.target.value })
                  }
                />
              </Field>
              <Field label="权重" htmlFor="channel-weight">
                <Input
                  id="channel-weight"
                  type="number"
                  min={1}
                  max={1000}
                  value={draft.weight}
                  onChange={(event) => update({ weight: event.target.value })}
                />
              </Field>
              <Field label="超时 (ms)" htmlFor="channel-timeout">
                <Input
                  id="channel-timeout"
                  type="number"
                  min={1}
                  value={draft.timeoutMs}
                  onChange={(event) =>
                    update({ timeoutMs: event.target.value })
                  }
                  placeholder="默认"
                />
              </Field>
            </div>

            <Field
              label="代理 URL"
              htmlFor="channel-proxy"
              hint="支持 http:// 与 socks5://，留空表示直连。"
            >
              <Input
                id="channel-proxy"
                value={draft.proxyUrl}
                onChange={(event) => update({ proxyUrl: event.target.value })}
                placeholder="http://127.0.0.1:7890"
                className="font-mono"
              />
            </Field>

            <div className="flex items-center justify-between rounded-lg border p-3">
              <div>
                <p className="text-sm font-medium">启用渠道</p>
                <p className="text-xs text-muted-foreground">
                  停用后不会参与路由。
                </p>
              </div>
              <Switch
                checked={draft.enabled}
                onCheckedChange={(checked) => update({ enabled: checked })}
              />
            </div>
          </div>

          <SheetFooter className="flex-row justify-end border-t p-4">
            <Button
              type="button"
              variant="outline"
              onClick={() => setOpen(false)}
            >
              取消
            </Button>
            <Button
              type="button"
              onClick={submit}
              disabled={saveMutation.isPending}
            >
              {saveMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : null}
              保存
            </Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>

      <Dialog open={modelDialogOpen} onOpenChange={setModelDialogOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>从上游拉取的模型</DialogTitle>
            <DialogDescription>
              选择要写入渠道的模型，保存表单后生效。
            </DialogDescription>
          </DialogHeader>
          <div className="max-h-[320px] space-y-1 overflow-y-auto rounded-lg border p-1">
            {fetchedModels.map((model) => {
              const selected = selectedModels.includes(model);
              return (
                <button
                  key={model}
                  type="button"
                  onClick={() => toggleModel(model)}
                  className={
                    selected
                      ? 'flex w-full items-center justify-between gap-2 rounded-md bg-accent px-2 py-1.5 text-left font-mono text-xs'
                      : 'flex w-full items-center justify-between gap-2 rounded-md px-2 py-1.5 text-left font-mono text-xs text-muted-foreground hover:bg-muted'
                  }
                >
                  <span className="truncate">{model}</span>
                  <span
                    className={
                      selected
                        ? 'size-2 shrink-0 rounded-full bg-primary'
                        : 'size-2 shrink-0 rounded-full border'
                    }
                  />
                </button>
              );
            })}
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setModelDialogOpen(false)}
            >
              取消
            </Button>
            <Button
              type="button"
              onClick={() => {
                update({ models: selectedModels });
                setModelDialogOpen(false);
                toast.success('已填入表单，保存后生效');
              }}
            >
              填入表单
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={deleteTarget != null}
        onOpenChange={(next) => {
          if (!next) {
            setDeleteTarget(null);
          }
        }}
        title={`删除渠道「${deleteTarget?.name ?? ''}」？`}
        description="删除后不可恢复，正在进行的请求不会中断。"
        confirmLabel="删除"
        destructive
        loading={deleteMutation.isPending}
        onConfirm={() => {
          if (deleteTarget) {
            deleteMutation.mutate(deleteTarget.id);
          }
        }}
      />
    </>
  );
}
