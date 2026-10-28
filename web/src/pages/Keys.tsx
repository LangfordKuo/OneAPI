import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Loader2, KeyRound, Plus, Trash2 } from 'lucide-react';
import dayjs from 'dayjs';
import { useState } from 'react';
import { toast } from 'sonner';
import { api } from '@/api/client';
import type { ApiKey } from '@/api/types';
import { EnabledBadge } from '@/components/common/badges';
import { CodeBlock } from '@/components/common/code-block';
import { ConfirmDialog } from '@/components/common/confirm-dialog';
import { EmptyState } from '@/components/common/empty-state';
import { Field } from '@/components/common/field';
import { PageHeader } from '@/components/common/page-header';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
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
import { formatDateTime } from '@/lib/format';

interface CreatePayload {
  name: string;
  expires_at: number | null;
  enabled: boolean;
}

function formatTime(value: number | null) {
  return value ? formatDateTime(value, 'YYYY-MM-DD HH:mm') : '-';
}

export default function Keys() {
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [name, setName] = useState('');
  const [expiresAt, setExpiresAt] = useState('');
  const [enabled, setEnabled] = useState(true);
  const [nameError, setNameError] = useState('');
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<ApiKey | null>(null);

  const keysQuery = useQuery({
    queryKey: ['keys'],
    queryFn: () => api.get<ApiKey[]>('/api/keys'),
  });

  const createMutation = useMutation({
    mutationFn: (payload: CreatePayload) =>
      api.post<ApiKey>('/api/keys', payload),
    onSuccess: (data) => {
      setCreateOpen(false);
      setName('');
      setExpiresAt('');
      setEnabled(true);
      if (data.key) {
        setCreatedKey(data.key);
      }
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, enabled: next }: { id: number; enabled: boolean }) =>
      api.put<ApiKey>(`/api/keys/${id}`, { enabled: next }),
    onSuccess: () => {
      toast.success('已更新');
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/api/keys/${id}`),
    onSuccess: () => {
      toast.success('已删除');
      setDeleteTarget(null);
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      toast.error(error instanceof Error ? error.message : String(error)),
  });

  function openCreate() {
    setName('');
    setExpiresAt('');
    setEnabled(true);
    setNameError('');
    setCreateOpen(true);
  }

  function submit() {
    if (!name.trim()) {
      setNameError('请输入名称');
      return;
    }
    createMutation.mutate({
      name: name.trim(),
      expires_at: expiresAt ? dayjs(expiresAt).valueOf() : null,
      enabled,
    });
  }

  const keys = keysQuery.data ?? [];

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
                  {keys.map((key) => (
                    <TableRow key={key.id}>
                      <TableCell className="font-medium">{key.name}</TableCell>
                      <TableCell className="hidden sm:table-cell">
                        <code className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs">
                          {key.key_prefix}…
                        </code>
                      </TableCell>
                      <TableCell>
                        <EnabledBadge enabled={key.enabled} />
                      </TableCell>
                      <TableCell className="hidden text-xs text-muted-foreground md:table-cell">
                        {formatTime(key.expires_at)}
                      </TableCell>
                      <TableCell className="hidden text-xs text-muted-foreground lg:table-cell">
                        {formatTime(key.last_used_at)}
                      </TableCell>
                      <TableCell className="hidden text-xs text-muted-foreground lg:table-cell">
                        {formatTime(key.created_at)}
                      </TableCell>
                      <TableCell>
                        <div className="flex items-center justify-end gap-3">
                          <Switch
                            checked={key.enabled}
                            disabled={
                              updateMutation.isPending &&
                              updateMutation.variables?.id === key.id
                            }
                            onCheckedChange={(checked) =>
                              updateMutation.mutate({
                                id: key.id,
                                enabled: checked,
                              })
                            }
                          />
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
                  ))}
                </TableBody>
              </Table>
            </div>
          )}
        </CardContent>
      </Card>

      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>新建 API Key</DialogTitle>
          </DialogHeader>
          <div className="space-y-4">
            <Field label="名称" htmlFor="key-name" error={nameError}>
              <Input
                id="key-name"
                value={name}
                onChange={(event) => {
                  setName(event.target.value);
                  setNameError('');
                }}
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
                value={expiresAt}
                onChange={(event) => setExpiresAt(event.target.value)}
              />
            </Field>
            <div className="flex items-center justify-between rounded-lg border p-3">
              <div>
                <p className="text-sm font-medium">启用</p>
                <p className="text-xs text-muted-foreground">
                  停用后该 Key 立即失效。
                </p>
              </div>
              <Switch checked={enabled} onCheckedChange={setEnabled} />
            </div>
          </div>
          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => setCreateOpen(false)}
            >
              取消
            </Button>
            <Button
              type="button"
              onClick={submit}
              disabled={createMutation.isPending}
            >
              {createMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : null}
              创建
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
