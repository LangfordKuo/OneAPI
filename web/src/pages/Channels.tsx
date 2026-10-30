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
  ChannelProtocolMode,
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
import { Card, CardContent } from '@/components/ui/card';
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

interface ProtocolDraft {
  mode: ChannelProtocolMode;
  target: ChannelKind | '';
  baseUrl: string;
  apiKey: string;
  authHeader: 'bearer' | 'x-api-key';
  models: string[];
  modelMapping: MappingRow[];
  extraHeaders: MappingRow[];
}

interface ChannelDraft {
  name: string;
  protocols: Record<ChannelKind, ProtocolDraft>;
  priority: string;
  weight: string;
  timeoutMs: string;
  proxyUrl: string;
  enabled: boolean;
}

const PROTOCOL_ORDER: ChannelKind[] = [
  'openai_chat',
  'openai_responses',
  'anthropic',
];

const KIND_LABELS: Record<ChannelKind, string> = {
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  anthropic: 'Anthropic',
};

const AUTH_HEADER_OPTIONS = [
  { value: 'bearer', label: 'Authorization: Bearer' },
  { value: 'x-api-key', label: 'x-api-key' },
];

const NO_CONVERSION = 'none';

function defaultAuthHeader(kind: ChannelKind): 'bearer' | 'x-api-key' {
  return kind === 'anthropic' ? 'x-api-key' : 'bearer';
}

function defaultProtocol(kind: ChannelKind): ProtocolDraft {
  return {
    mode: 'native',
    target: '',
    baseUrl: '',
    apiKey: '',
    authHeader: defaultAuthHeader(kind),
    models: [],
    modelMapping: [],
    extraHeaders: [],
  };
}

function emptyDraft(): ChannelDraft {
  return {
    name: '',
    protocols: {
      openai_chat: defaultProtocol('openai_chat'),
      openai_responses: {
        ...defaultProtocol('openai_responses'),
        mode: 'convert',
        target: 'openai_chat',
      },
      anthropic: {
        ...defaultProtocol('anthropic'),
        mode: 'convert',
        target: 'openai_chat',
      },
    },
    priority: '0',
    weight: '1',
    timeoutMs: '',
    proxyUrl: '',
    enabled: true,
  };
}

function rowsFromObject(value: Record<string, string>): MappingRow[] {
  return Object.entries(value ?? {}).map(([key, itemValue]) => ({
    key,
    value: itemValue,
  }));
}

function objectFromRows(rows: MappingRow[]): Record<string, string> {
  return Object.fromEntries(
    rows
      .filter((row) => row.key.trim())
      .map((row) => [row.key.trim(), row.value.trim()]),
  );
}

function protocolDraft(
  channel: Channel,
  kind: ChannelKind,
): ProtocolDraft {
  const stored = channel.protocols?.find((item) => item.protocol === kind);
  if (stored) {
    return {
      mode: stored.mode,
      target: stored.mode === 'convert' ? (stored.target ?? '') : '',
      baseUrl: stored.base_url,
      apiKey: '',
      authHeader: stored.auth_header,
      models: stored.models,
      modelMapping: rowsFromObject(stored.model_mapping),
      extraHeaders: rowsFromObject(stored.extra_headers),
    };
  }

  const native = (channel.kinds ?? []).includes(kind);
  return {
    mode: native ? 'native' : 'convert',
    target: native ? '' : channel.fallback_kind,
    baseUrl: native ? channel.base_url : '',
    apiKey: '',
    authHeader: native ? channel.auth_header : defaultAuthHeader(kind),
    models: native ? channel.models : [],
    modelMapping: native ? rowsFromObject(channel.model_mapping) : [],
    extraHeaders: native ? rowsFromObject(channel.extra_headers) : [],
  };
}

function normalizeProtocols(
  protocols: Record<ChannelKind, ProtocolDraft>,
): Record<ChannelKind, ProtocolDraft> {
  const next = { ...protocols };
  const nativeKinds = PROTOCOL_ORDER.filter(
    (kind) => next[kind].mode === 'native',
  );
  if (nativeKinds.length === 0) {
    next.openai_chat = { ...next.openai_chat, mode: 'native', target: '' };
    nativeKinds.push('openai_chat');
  }

  for (const kind of PROTOCOL_ORDER) {
    const protocol = next[kind];
    if (protocol.mode !== 'convert') {
      if (protocol.target) {
        next[kind] = { ...protocol, target: '' };
      }
      continue;
    }
    const target = protocol.target;
    const valid =
      target !== '' &&
      target !== kind &&
      next[target].mode === 'native' &&
      target !== undefined;
    if (!valid) {
      const fallback = nativeKinds.find((item) => item !== kind);
      next[kind] = { ...protocol, target: fallback ?? '' };
      if (!fallback) {
        next[kind] = { ...protocol, mode: 'native', target: '' };
      }
    }
  }
  return next;
}

function draftFromChannel(channel: Channel): ChannelDraft {
  const protocols = Object.fromEntries(
    PROTOCOL_ORDER.map((kind) => [kind, protocolDraft(channel, kind)]),
  ) as Record<ChannelKind, ProtocolDraft>;

  return {
    name: channel.name,
    protocols: normalizeProtocols(protocols),
    priority: String(channel.priority),
    weight: String(channel.weight),
    timeoutMs: channel.timeout_ms == null ? '' : String(channel.timeout_ms),
    proxyUrl: channel.proxy_url ?? '',
    enabled: channel.enabled,
  };
}

function toNumber(value: string, fallback: number): number {
  // `Number('')` is 0, which would turn a cleared weight field into weight 0 and
  // get rejected by the backend, so an empty input means "use the default".
  if (!value.trim()) {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function toPayload(draft: ChannelDraft) {
  return {
    name: draft.name.trim(),
    protocols: PROTOCOL_ORDER.map((kind) => {
      const protocol = draft.protocols[kind];
      return {
        protocol: kind,
        mode: protocol.mode,
        target:
          protocol.mode === 'convert' && protocol.target
            ? protocol.target
            : null,
        base_url: protocol.baseUrl.trim(),
        api_key: protocol.apiKey.trim() || undefined,
        auth_header: protocol.authHeader,
        models: protocol.models,
        model_mapping: objectFromRows(protocol.modelMapping),
        extra_headers: objectFromRows(protocol.extraHeaders),
      };
    }),
    priority: toNumber(draft.priority, 0),
    weight: toNumber(draft.weight, 1),
    timeout_ms: draft.timeoutMs.trim()
      ? Math.max(1, toNumber(draft.timeoutMs, 1))
      : null,
    proxy_url: draft.proxyUrl.trim() || null,
    enabled: draft.enabled,
  };
}

function existingKey(channel: Channel | null, kind: ChannelKind): string {
  if (!channel) {
    return '';
  }
  return (
    channel.protocols?.find((item) => item.protocol === kind)?.api_key_masked ??
    (channel.kinds.includes(kind) ? channel.api_key_masked : '')
  );
}

function validateDraft(
  draft: ChannelDraft,
  editing: Channel | null,
): Record<string, string> {
  const errors: Record<string, string> = {};
  if (!draft.name.trim()) {
    errors.name = '请输入渠道名称';
  }

  const nativeKinds = PROTOCOL_ORDER.filter(
    (kind) => draft.protocols[kind].mode === 'native',
  );
  if (nativeKinds.length === 0) {
    errors.protocols = '至少保留一个「不转换」的协议';
  }

  for (const kind of PROTOCOL_ORDER) {
    const protocol = draft.protocols[kind];
    if (protocol.mode === 'native') {
      if (!protocol.baseUrl.trim()) {
        errors[`protocols.${kind}.baseUrl`] = '请输入 Base URL';
      }
      if (protocol.models.length === 0) {
        errors[`protocols.${kind}.models`] = '至少填写一个模型';
      }
      if (!protocol.apiKey.trim() && !existingKey(editing, kind)) {
        errors[`protocols.${kind}.apiKey`] = '请输入上游 API Key';
      }
    } else {
      if (!protocol.target) {
        errors[`protocols.${kind}.target`] = '请选择转换目标';
      } else if (draft.protocols[protocol.target].mode !== 'native') {
        errors[`protocols.${kind}.target`] =
          '转换目标必须是不转换的协议';
      }
    }
  }

  return errors;
}

function channelModels(channel: Channel): string[] {
  const protocols = channel.protocols ?? [];
  const native = protocols.filter((item) => item.mode === 'native');
  const source = native.length > 0 ? native : protocols;
  return Array.from(new Set(source.flatMap((item) => item.models)));
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
  const [activeProtocol, setActiveProtocol] =
    useState<ChannelKind>('openai_chat');
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [deleteTarget, setDeleteTarget] = useState<Channel | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [modelTarget, setModelTarget] =
    useState<ChannelKind>('openai_chat');
  const [modelDialogOpen, setModelDialogOpen] = useState(false);

  const channelsQuery = useQuery({
    queryKey: ['channels'],
    queryFn: () => api.get<Channel[]>('/api/channels'),
  });

  function clearProtocolErrors(kind: ChannelKind) {
    setErrors((current) => {
      const next = { ...current };
      for (const key of Object.keys(next)) {
        if (key.startsWith(`protocols.${kind}.`)) {
          delete next[key];
        }
      }
      return next;
    });
  }

  function updateShared(patch: Partial<ChannelDraft>) {
    setDraft((current) => ({ ...current, ...patch }));
    setErrors((current) => {
      const next = { ...current };
      for (const key of Object.keys(patch)) {
        delete next[key];
      }
      return next;
    });
  }

  function updateProtocol(kind: ChannelKind, patch: Partial<ProtocolDraft>) {
    setDraft((current) => {
      const next = {
        ...current,
        protocols: {
          ...current.protocols,
          [kind]: { ...current.protocols[kind], ...patch },
        },
      };
      return { ...next, protocols: normalizeProtocols(next.protocols) };
    });
    clearProtocolErrors(kind);
  }

  function conversionOptions(kind: ChannelKind) {
    return PROTOCOL_ORDER.filter(
      (item) =>
        item !== kind && draft.protocols[item].mode === 'native',
    );
  }

  function setConversion(kind: ChannelKind, value: string) {
    if (value === NO_CONVERSION) {
      updateProtocol(kind, { mode: 'native', target: '' });
      return;
    }

    const target = value as ChannelKind;
    if (
      target === kind ||
      draft.protocols[target].mode !== 'native'
    ) {
      return;
    }

    setDraft((current) => {
      const protocols = {
        ...current.protocols,
        [kind]: {
          ...current.protocols[kind],
          mode: 'convert' as ChannelProtocolMode,
          target,
        },
      };
      for (const item of PROTOCOL_ORDER) {
        if (
          item !== kind &&
          protocols[item].mode === 'convert' &&
          protocols[item].target === kind
        ) {
          protocols[item] = { ...protocols[item], target };
        }
      }
      return { ...current, protocols: normalizeProtocols(protocols) };
    });
    clearProtocolErrors(kind);
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

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      api.put<Channel>(`/api/channels/${id}/enabled`, { enabled }),
    onSuccess: (channel) => {
      toast.success(channel.enabled ? '渠道已启用' : '渠道已停用');
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

  function showErrors(nextErrors: Record<string, string>) {
    setErrors(nextErrors);
    const firstProtocol = PROTOCOL_ORDER.find((kind) =>
      Object.keys(nextErrors).some((key) =>
        key.startsWith(`protocols.${kind}.`),
      ),
    );
    if (firstProtocol) {
      setActiveProtocol(firstProtocol);
    }
  }

  function probe(action: 'test' | 'fetch-models') {
    const nextErrors = validateDraft(draft, editing);
    if (Object.keys(nextErrors).length > 0) {
      showErrors(nextErrors);
      toast.error('请先补全表单必填项');
      return;
    }

    const payload = {
      ...toPayload(draft),
      id: editing?.id,
      probe_protocol: activeProtocol,
    };
    if (action === 'test') {
      probeTestMutation.mutate(payload);
    } else {
      setModelTarget(activeProtocol);
      probeFetchMutation.mutate(payload);
    }
  }

  function openCreate() {
    setEditing(null);
    setDraft(emptyDraft());
    setActiveProtocol('openai_chat');
    setErrors({});
    setOpen(true);
  }

  function openEdit(channel: Channel) {
    setEditing(channel);
    setDraft(draftFromChannel(channel));
    const primary =
      channel.protocols?.find((item) => item.mode === 'native')?.protocol ??
      channel.kinds[0] ??
      'openai_chat';
    setActiveProtocol(primary);
    setErrors({});
    setOpen(true);
  }

  function submit() {
    const nextErrors = validateDraft(draft, editing);
    if (Object.keys(nextErrors).length > 0) {
      showErrors(nextErrors);
      return;
    }
    saveMutation.mutate({
      id: editing?.id,
      payload: toPayload(draft),
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
  const active = draft.protocols[activeProtocol];
  const savedProtocol = editing?.protocols?.find(
    (item) => item.protocol === activeProtocol,
  );
  const targets = conversionOptions(activeProtocol);
  const targetItems = [
    { value: NO_CONVERSION, label: '不转换' },
    ...targets.map((kind) => ({ value: kind, label: KIND_LABELS[kind] })),
  ];
  const conversionValue =
    active.mode === 'convert' && active.target
      ? active.target
      : NO_CONVERSION;

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
                    const models = channelModels(channel);
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
                            <span className="text-sm">{models.length} 个</span>
                            {models.includes('*') ? (
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
                          <div className="flex items-center justify-end gap-1">
                            <Switch
                              checked={channel.enabled}
                              disabled={
                                toggleMutation.isPending &&
                                toggleMutation.variables?.id === channel.id
                              }
                              aria-label={
                                channel.enabled ? '停用渠道' : '启用渠道'
                              }
                              onCheckedChange={(checked) =>
                                toggleMutation.mutate({
                                  id: channel.id,
                                  enabled: checked,
                                })
                              }
                            />
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
              使用当前协议页的配置，无需先保存
            </span>
          </div>

          <div className="flex-1 space-y-4 overflow-y-auto p-4">
            <Field label="名称" htmlFor="channel-name" error={errors.name}>
              <Input
                id="channel-name"
                value={draft.name}
                onChange={(event) =>
                  updateShared({ name: event.target.value })
                }
                placeholder="例如：OpenAI 主账号"
              />
            </Field>

            <Field
              label="支持的协议"
              error={errors.protocols}
              hint="每个协议页单独配置上游；选择转换时复用目标协议的地址与密钥。"
            >
              <div className="flex flex-wrap gap-2">
                {PROTOCOL_ORDER.map((kind) => {
                  const selected = activeProtocol === kind;
                  const hasError = Object.keys(errors).some((key) =>
                    key.startsWith(`protocols.${kind}.`),
                  );
                  return (
                    <Button
                      key={kind}
                      type="button"
                      variant={selected ? 'default' : 'outline'}
                      size="sm"
                      onClick={() => setActiveProtocol(kind)}
                    >
                      {KIND_LABELS[kind]}
                      {hasError ? (
                        <span className="size-1.5 rounded-full bg-destructive" />
                      ) : null}
                    </Button>
                  );
                })}
              </div>
            </Field>

            <Field
              label="转换目标"
              error={errors[`protocols.${activeProtocol}.target`]}
              hint="选「不转换」时使用本页配置；选某个协议时，入站请求会转换成该协议并使用它的配置。"
            >
              <Select
                items={targetItems}
                value={conversionValue}
                onValueChange={(value) => {
                  if (value) {
                    setConversion(activeProtocol, value);
                  }
                }}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="选择转换目标" />
                </SelectTrigger>
                <SelectContent>
                  {targetItems.map((item) => (
                    <SelectItem key={item.value} value={item.value}>
                      {item.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>

            {active.mode === 'native' ? (
              <>
                <Field
                  label="Base URL"
                  htmlFor="channel-url"
                  error={errors[`protocols.${activeProtocol}.baseUrl`]}
                >
                  <Input
                    id="channel-url"
                    value={active.baseUrl}
                    onChange={(event) =>
                      updateProtocol(activeProtocol, {
                        baseUrl: event.target.value,
                      })
                    }
                    placeholder="https://api.openai.com/v1"
                    className="font-mono"
                  />
                </Field>

                <Field
                  label="上游 API Key"
                  htmlFor="channel-key"
                  error={errors[`protocols.${activeProtocol}.apiKey`]}
                  hint={
                    savedProtocol?.api_key_masked
                      ? `当前：${savedProtocol.api_key_masked}`
                      : undefined
                  }
                >
                  <Input
                    id="channel-key"
                    type="password"
                    autoComplete="new-password"
                    value={active.apiKey}
                    onChange={(event) =>
                      updateProtocol(activeProtocol, {
                        apiKey: event.target.value,
                      })
                    }
                    placeholder={
                      savedProtocol?.api_key_masked
                        ? '留空保持不变'
                        : 'sk-...'
                    }
                    className="font-mono"
                  />
                </Field>

                <Field label="鉴权头">
                  <Select
                    items={AUTH_HEADER_OPTIONS}
                    value={active.authHeader}
                    onValueChange={(value) => {
                      if (value) {
                        updateProtocol(activeProtocol, {
                          authHeader: value as 'bearer' | 'x-api-key',
                        });
                      }
                    }}
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {AUTH_HEADER_OPTIONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </Field>

                <Field
                  label="模型"
                  error={errors[`protocols.${activeProtocol}.models`]}
                  hint="输入模型名后回车；* 表示接受任意模型。"
                >
                  <TagInput
                    value={active.models}
                    onChange={(models) =>
                      updateProtocol(activeProtocol, { models })
                    }
                    placeholder="gpt-4o-mini"
                  />
                </Field>

                <Field
                  label="模型映射"
                  hint="客户端模型 → 上游模型；未命中的模型原样转发。"
                >
                  <KeyValueEditor
                    value={active.modelMapping}
                    onChange={(modelMapping) =>
                      updateProtocol(activeProtocol, { modelMapping })
                    }
                    keyPlaceholder="客户端模型"
                    valuePlaceholder="上游模型"
                  />
                </Field>

                <Field label="自定义请求头">
                  <KeyValueEditor
                    value={active.extraHeaders}
                    onChange={(extraHeaders) =>
                      updateProtocol(activeProtocol, { extraHeaders })
                    }
                    keyPlaceholder="Header 名称"
                    valuePlaceholder="Header 值"
                  />
                </Field>
              </>
            ) : null}

            <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
              <Field label="优先级" htmlFor="channel-priority">
                <Input
                  id="channel-priority"
                  type="number"
                  min={-1000}
                  max={1000}
                  value={draft.priority}
                  onChange={(event) =>
                    updateShared({ priority: event.target.value })
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
                  onChange={(event) =>
                    updateShared({ weight: event.target.value })
                  }
                />
              </Field>
              <Field label="超时 (ms)" htmlFor="channel-timeout">
                <Input
                  id="channel-timeout"
                  type="number"
                  min={1}
                  value={draft.timeoutMs}
                  onChange={(event) =>
                    updateShared({ timeoutMs: event.target.value })
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
                onChange={(event) =>
                  updateShared({ proxyUrl: event.target.value })
                }
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
                onCheckedChange={(checked) =>
                  updateShared({ enabled: checked })
                }
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
              选择要写入「{KIND_LABELS[modelTarget]}」页的模型，保存表单后生效。
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
                updateProtocol(modelTarget, { models: selectedModels });
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
