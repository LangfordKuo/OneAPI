import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  KeyRound,
  Loader2,
  Pencil,
  Plus,
  Trash2,
} from 'lucide-react';
import dayjs from 'dayjs';
import { useState } from 'react';
import { toast } from 'sonner';
import { api } from '@/api/client';
import type { ApiKey, Channel } from '@/api/types';
import { EnabledBadge } from '@/components/common/badges';
import { CodeBlock } from '@/components/common/code-block';
import { ConfirmDialog } from '@/components/common/confirm-dialog';
import { EmptyState } from '@/components/common/empty-state';
import { Field } from '@/components/common/field';
import { PageHeader } from '@/components/common/page-header';
import { TagInput } from '@/components/common/tag-input';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Card, CardContent } from '@/components/ui/card';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
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
import { formatCompactNumber, formatDateTime } from '@/lib/format';
import { errorMessage } from '@/lib/error';

interface KeyDraft {
  name: string;
  expiresAt: string;
  enabled: boolean;
  channelIds: number[];
  models: string[];
  tokenLimit: string;
  dailyTokenLimit: string;
}

function emptyDraft(): KeyDraft {
  return {
    name: '',
    expiresAt: '',
    enabled: true,
    channelIds: [],
    models: [],
    tokenLimit: '',
    dailyTokenLimit: '',
  };
}

function draftFromKey(key: ApiKey, channels: Channel[]): KeyDraft {
  const existingChannels = new Set(channels.map((channel) => channel.id));
  return {
    name: key.name,
    expiresAt: key.expires_at
      ? dayjs(key.expires_at).format('YYYY-MM-DDTHH:mm')
      : '',
    enabled: key.enabled,
    channelIds:
      channels.length > 0
        ? key.allowed_channel_ids.filter((id) => existingChannels.has(id))
        : key.allowed_channel_ids,
    models: key.allowed_models,
    tokenLimit: key.token_limit ? String(key.token_limit) : '',
    dailyTokenLimit: key.daily_token_limit ? String(key.daily_token_limit) : '',
  };
}

/** Parse an optional token limit for the request payload. */
function limitOf(raw: string): number | null {
  return raw.trim() ? Number(raw.trim()) : null;
}

/** Optional positive-integer limit; an empty field means "no limit". */
function validateTokenLimit(
  raw: string,
  field: 'tokenLimit' | 'dailyTokenLimit',
  errors: Record<string, string>,
) {
  const value = raw.trim();
  if (!value) {
    return;
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    errors[field] = '请输入正整数，或留空表示不限制';
  }
}

function restrictionSummary(key: ApiKey): string {
  const channels =
    key.allowed_channel_ids.length === 0
      ? '全部渠道'
      : `${key.allowed_channel_ids.length} 个渠道`;
  const models =
    key.allowed_models.length === 0 || key.allowed_models.includes('*')
      ? '全部模型'
      : `${key.allowed_models.length} 个模型`;
  return `${channels} · ${models}`;
}

export default function Keys() {
  const queryClient = useQueryClient();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [editingKey, setEditingKey] = useState<ApiKey | null>(null);
  const [draft, setDraft] = useState<KeyDraft>(emptyDraft);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ApiKey | null>(null);

  const keysQuery = useQuery({
    queryKey: ['keys'],
    queryFn: () => api.get<ApiKey[]>('/api/keys'),
  });
  const channelsQuery = useQuery({
    queryKey: ['channels'],
    queryFn: () => api.get<Channel[]>('/api/channels'),
  });

  const saveMutation = useMutation({
    mutationFn: ({ id, payload }: { id?: number; payload: unknown }) =>
      id
        ? api.put<ApiKey>(`/api/keys/${id}`, payload)
        : api.post<ApiKey>('/api/keys', payload),
    onSuccess: (data, variables) => {
      setDialogOpen(false);
      setEditingKey(null);
      if (!variables.id && data.key) {
        setCreatedKey(data.key);
      } else {
        toast.success('已保存');
      }
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const toggleMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      api.put<ApiKey>(`/api/keys/${id}`, { enabled }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/api/keys/${id}`),
    onSuccess: () => {
      toast.success('已删除');
      setDeleteTarget(null);
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  function update(patch: Partial<KeyDraft>) {
    setDraft((current) => ({ ...current, ...patch }));
    setErrors((current) => {
      const next = { ...current };
      for (const key of Object.keys(patch)) {
        delete next[key];
      }
      return next;
    });
  }

  function toggleChannel(id: number) {
    update({
      channelIds: draft.channelIds.includes(id)
        ? draft.channelIds.filter((item) => item !== id)
        : [...draft.channelIds, id],
    });
  }

  function openCreate() {
    setEditingKey(null);
    setDraft(emptyDraft());
    setErrors({});
    setDialogOpen(true);
  }

  function openEdit(key: ApiKey) {
    setEditingKey(key);
    setDraft(draftFromKey(key, channelsQuery.data ?? []));
    setErrors({});
    setDialogOpen(true);
  }

  function submit() {
    const nextErrors: Record<string, string> = {};
    if (!draft.name.trim()) {
      nextErrors.name = '请输入名称';
    }
    validateTokenLimit(draft.tokenLimit, 'tokenLimit', nextErrors);
    validateTokenLimit(draft.dailyTokenLimit, 'dailyTokenLimit', nextErrors);
    setErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) {
      return;
    }

    const channels = channelsQuery.data;
    const channelIds = channels
      ? draft.channelIds.filter((id) =>
          channels.some((channel) => channel.id === id),
        )
      : draft.channelIds;

    saveMutation.mutate({
      id: editingKey?.id,
      payload: {
        name: draft.name.trim(),
        expires_at: draft.expiresAt ? dayjs(draft.expiresAt).valueOf() : null,
        enabled: draft.enabled,
        allowed_channel_ids: channelIds,
        allowed_models: draft.models,
        token_limit: limitOf(draft.tokenLimit),
        daily_token_limit: limitOf(draft.dailyTokenLimit),
      },
    });
  }

  const keys = keysQuery.data ?? [];
  const channels = channelsQuery.data ?? [];

  return (
    <>
      <PageHeader
        title="API Keys"
        description="客户端使用这些 Key 访问网关。完整 Key 只在创建时显示一次。"
        actions={
          <Button onClick={openCreate}>
            <Plus />
            新建 API Key
          </Button>
        }
      />

      <Card className="overflow-hidden">
        <CardContent className="p-0">
          {keysQuery.isLoading ? (
            <div className="space-y-2 p-4">
              {Array.from({ length: 4 }).map((_, index) => (
                <Skeleton key={index} className="h-12 w-full" />
              ))}
            </div>
          ) : keys.length === 0 ? (
            <div className="p-6">
              <EmptyState
                title="还没有 API Key"
                description="创建一个 Key，把它配置到客户端即可开始调用。"
                icon={<KeyRound className="size-5" />}
                action={
                  <Button onClick={openCreate}>
                    <Plus />
                    新建 API Key
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
                    <TableHead className="hidden sm:table-cell">
                      Key 前缀
                    </TableHead>
                    <TableHead>用量</TableHead>
                    <TableHead>状态</TableHead>
                    <TableHead className="hidden md:table-cell">
                      过期时间
                    </TableHead>
                    <TableHead className="hidden lg:table-cell">
                      最后使用
                    </TableHead>
                    <TableHead className="hidden lg:table-cell">
                      创建时间
                    </TableHead>
                    <TableHead className="text-right">操作</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {keys.map((key) => {
                    const percent =
                      key.token_limit && key.token_limit > 0
                        ? Math.min(
                            100,
                            Math.round(
                              (key.token_usage / key.token_limit) * 100,
                            ),
                          )
                        : null;
                    const overLimit =
                      key.token_limit != null &&
                      key.token_limit > 0 &&
                      key.token_usage >= key.token_limit;

                    return (
                      <TableRow key={key.id}>
                        <TableCell>
                          <div className="space-y-0.5">
                            <p className="font-medium">{key.name}</p>
                            <p className="text-xs text-muted-foreground">
                              {restrictionSummary(key)}
                            </p>
                          </div>
                        </TableCell>
                        <TableCell className="hidden sm:table-cell">
                          <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
                            {key.key_prefix}…
                          </code>
                        </TableCell>
                        <TableCell>
                          <div className="space-y-1">
                            <p className="whitespace-nowrap text-xs tabular-nums">
                              {formatCompactNumber(key.token_usage)}
                              {key.token_limit
                                ? ` / ${formatCompactNumber(key.token_limit)}`
                                : ''}
                            </p>
                            {percent !== null ? (
                              <div className="h-1 w-24 overflow-hidden rounded-full bg-muted">
                                <div
                                  className={
                                    overLimit
                                      ? 'h-full bg-destructive'
                                      : 'h-full bg-primary'
                                  }
                                  style={{ width: `${percent}%` }}
                                />
                              </div>
                            ) : null}
                            {key.daily_token_limit ? (
                              <p className="whitespace-nowrap text-[11px] text-muted-foreground">
                                今日 {formatCompactNumber(key.daily_token_usage)}{' '}
                                / {formatCompactNumber(key.daily_token_limit)}
                              </p>
                            ) : null}
                          </div>
                        </TableCell>
                        <TableCell>
                          <div className="flex flex-wrap items-center gap-1">
                            <EnabledBadge enabled={key.enabled} />
                            {overLimit ? (
                              <Badge
                                variant="outline"
                                className="border-destructive/30 bg-destructive/10 text-xs text-destructive"
                              >
                                已超限
                              </Badge>
                            ) : null}
                          </div>
                        </TableCell>
                        <TableCell className="hidden text-xs text-muted-foreground md:table-cell">
                          {formatDateTime(key.expires_at, 'YYYY-MM-DD HH:mm')}
                        </TableCell>
                        <TableCell className="hidden text-xs text-muted-foreground lg:table-cell">
                          {formatDateTime(key.last_used_at, 'YYYY-MM-DD HH:mm')}
                        </TableCell>
                        <TableCell className="hidden text-xs text-muted-foreground lg:table-cell">
                          {formatDateTime(key.created_at, 'YYYY-MM-DD HH:mm')}
                        </TableCell>
                        <TableCell>
                          <div className="flex items-center justify-end gap-2">
                            <Switch
                              checked={key.enabled}
                              disabled={
                                toggleMutation.isPending &&
                                toggleMutation.variables?.id === key.id
                              }
                              onCheckedChange={(checked) =>
                                toggleMutation.mutate({
                                  id: key.id,
                                  enabled: checked,
                                })
                              }
                            />
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() => openEdit(key)}
                            >
                              <Pencil />
                              <span className="hidden sm:inline">编辑</span>
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              className="text-destructive hover:text-destructive"
                              onClick={() => setDeleteTarget(key)}
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

      <Dialog
        open={dialogOpen}
        onOpenChange={(next) => {
          setDialogOpen(next);
          if (!next) {
            setEditingKey(null);
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>
              {editingKey ? '编辑 API Key' : '新建 API Key'}
            </DialogTitle>
          </DialogHeader>
          <div className="max-h-[65vh] space-y-4 overflow-y-auto pr-1">
            <Field label="名称" htmlFor="key-name" error={errors.name}>
              <Input
                id="key-name"
                value={draft.name}
                onChange={(event) => update({ name: event.target.value })}
                placeholder="例如：本地开发"
                onKeyDown={(event) => {
                  if (event.key === 'Enter') {
                    event.preventDefault();
                    submit();
                  }
                }}
              />
            </Field>
            <Field
              label="过期时间（可选）"
              htmlFor="key-expires"
              hint="留空表示永不过期。"
            >
              <Input
                id="key-expires"
                type="datetime-local"
                value={draft.expiresAt}
                onChange={(event) =>
                  update({ expiresAt: event.target.value })
                }
              />
            </Field>

            <Field
              label="可用渠道（可选）"
              hint="不选择任何渠道表示允许全部渠道；渠道被删除后会自动从限制中忽略。"
            >
              <div className="flex flex-wrap gap-2">
                {channels.length === 0 ? (
                  <span className="text-xs text-muted-foreground">
                    暂无渠道
                  </span>
                ) : (
                  channels.map((channel) => {
                    const active = draft.channelIds.includes(channel.id);
                    return (
                      <Button
                        key={channel.id}
                        type="button"
                        variant={active ? 'default' : 'outline'}
                        size="sm"
                        onClick={() => toggleChannel(channel.id)}
                      >
                        {channel.name}
                      </Button>
                    );
                  })
                )}
              </div>
              {draft.channelIds.length === 0 ? (
                <p className="text-xs text-muted-foreground">当前：全部渠道</p>
              ) : (
                <p className="text-xs text-muted-foreground">
                  当前：{draft.channelIds.length} 个渠道
                </p>
              )}
            </Field>

            <Field
              label="可用模型（可选）"
              hint="留空表示全部模型；支持 * 通配。"
            >
              <TagInput
                value={draft.models}
                onChange={(models) => update({ models })}
                placeholder="gpt-4o-mini"
              />
            </Field>

            <Field
              label="Token 上限（可选）"
              htmlFor="key-token-limit"
              error={errors.tokenLimit}
              hint="累计总 tokens 达到上限后自动停用；留空表示不限制。已超限的 Key 需要提高或清空上限后重新启用。"
            >
              <Input
                id="key-token-limit"
                type="number"
                min={1}
                value={draft.tokenLimit}
                onChange={(event) =>
                  update({ tokenLimit: event.target.value })
                }
                placeholder="留空不限制"
              />
            </Field>

            <Field
              label="每日 Token 上限（可选）"
              htmlFor="key-daily-token-limit"
              error={errors.dailyTokenLimit}
              hint="按 UTC 自然日统计，达到上限后当天返回 429，次日自动恢复；留空表示不限制。"
            >
              <Input
                id="key-daily-token-limit"
                type="number"
                min={1}
                value={draft.dailyTokenLimit}
                onChange={(event) =>
                  update({ dailyTokenLimit: event.target.value })
                }
                placeholder="留空不限制"
              />
            </Field>

            <div className="flex items-center justify-between rounded-lg border p-3">
              <div>
                <p className="text-sm font-medium">启用</p>
                <p className="text-xs text-muted-foreground">
                  停用后该 Key 立即失效。
                </p>
              </div>
              <Switch
                checked={draft.enabled}
                onCheckedChange={(checked) => update({ enabled: checked })}
              />
            </div>
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setDialogOpen(false)}
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
              {editingKey ? '保存' : '创建'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog
        open={createdKey !== null}
        onOpenChange={(next) => {
          if (!next) {
            setCreatedKey(null);
          }
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>API Key 已创建</DialogTitle>
          </DialogHeader>
          <Alert variant="destructive">
            <AlertTitle>完整 Key 只显示一次</AlertTitle>
            <AlertDescription>请立即复制并妥善保存。</AlertDescription>
          </Alert>
          <CodeBlock copyText={createdKey ?? ''}>
            {createdKey ?? ''}
          </CodeBlock>
          <DialogFooter>
            <Button type="button" onClick={() => setCreatedKey(null)}>
              我已保存
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
        title={`删除 Key「${deleteTarget?.name ?? ''}」？`}
        description="删除后使用该 Key 的客户端会立即失败。"
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
