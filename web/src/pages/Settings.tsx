import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Download, Loader2, RefreshCw, ShieldCheck, Upload } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { api, ApiError, requestRaw } from '@/api/client';
import type {
  Settings,
  UpdateApplyResult,
  UpdateInfo,
} from '@/api/types';
import { CodeBlock } from '@/components/common/code-block';
import { ConfirmDialog } from '@/components/common/confirm-dialog';
import { Field } from '@/components/common/field';
import { PageHeader } from '@/components/common/page-header';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Skeleton } from '@/components/ui/skeleton';
import { Switch } from '@/components/ui/switch';
import { errorMessage } from '@/lib/error';

interface SettingsDraft {
  site_name: string;
  site_domain: string;
  default_max_tokens: string;
  nonstream_timeout_seconds: string;
  stream_idle_timeout_seconds: string;
  usage_log_retention_days: string;
  response_retention_days: string;
  store_request_bodies: boolean;
  update_source: 'github' | 'ghproxy';
  client_ip_header: string;
}

const UPDATE_SOURCE_OPTIONS = [
  { value: 'github', label: 'GitHub 官方' },
  { value: 'ghproxy', label: '国内加速（gh-proxy.com）' },
];

/// Numeric defaults. Used both to seed an empty draft and as the fallback when
/// a field is cleared, so the two can never disagree.
const NUMERIC_DEFAULTS = {
  default_max_tokens: 4096,
  nonstream_timeout_seconds: 600,
  stream_idle_timeout_seconds: 120,
  usage_log_retention_days: 30,
  response_retention_days: 7,
} as const;

const EMPTY_DRAFT: SettingsDraft = {
  site_name: '',
  site_domain: '',
  default_max_tokens: String(NUMERIC_DEFAULTS.default_max_tokens),
  nonstream_timeout_seconds: String(
    NUMERIC_DEFAULTS.nonstream_timeout_seconds,
  ),
  stream_idle_timeout_seconds: String(
    NUMERIC_DEFAULTS.stream_idle_timeout_seconds,
  ),
  usage_log_retention_days: String(NUMERIC_DEFAULTS.usage_log_retention_days),
  response_retention_days: String(NUMERIC_DEFAULTS.response_retention_days),
  store_request_bodies: false,
  update_source: 'github',
  client_ip_header: '',
};

function toDraft(settings: Settings): SettingsDraft {
  return {
    site_name: settings.site_name,
    site_domain: settings.site_domain ?? '',
    default_max_tokens: String(settings.default_max_tokens),
    nonstream_timeout_seconds: String(settings.nonstream_timeout_seconds),
    stream_idle_timeout_seconds: String(settings.stream_idle_timeout_seconds),
    usage_log_retention_days: String(settings.usage_log_retention_days),
    response_retention_days: String(settings.response_retention_days),
    store_request_bodies: settings.store_request_bodies,
    update_source: settings.update_source,
    client_ip_header: settings.client_ip_header ?? '',
  };
}

function toInt(value: string, fallback: number): number {
  if (!value.trim()) {
    return fallback;
  }
  const parsed = Number(value);
  return Number.isFinite(parsed) ? Math.trunc(parsed) : fallback;
}

function toSettings(draft: SettingsDraft): Settings {
  return {
    site_name: draft.site_name.trim(),
    site_domain: draft.site_domain.trim(),
    default_max_tokens: toInt(
      draft.default_max_tokens,
      NUMERIC_DEFAULTS.default_max_tokens,
    ),
    nonstream_timeout_seconds: toInt(
      draft.nonstream_timeout_seconds,
      NUMERIC_DEFAULTS.nonstream_timeout_seconds,
    ),
    stream_idle_timeout_seconds: toInt(
      draft.stream_idle_timeout_seconds,
      NUMERIC_DEFAULTS.stream_idle_timeout_seconds,
    ),
    usage_log_retention_days: toInt(
      draft.usage_log_retention_days,
      NUMERIC_DEFAULTS.usage_log_retention_days,
    ),
    response_retention_days: toInt(
      draft.response_retention_days,
      NUMERIC_DEFAULTS.response_retention_days,
    ),
    store_request_bodies: draft.store_request_bodies,
    update_source: draft.update_source,
    client_ip_header: draft.client_ip_header.trim(),
  };
}

export default function SettingsPage() {
  const queryClient = useQueryClient();
  const [draftOverride, setDraftOverride] = useState<SettingsDraft | null>(
    null,
  );
  const [siteNameError, setSiteNameError] = useState('');
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateMessage, setUpdateMessage] = useState<string | null>(null);

  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [passwordErrors, setPasswordErrors] = useState<
    Record<string, string>
  >({});
  const [backupFile, setBackupFile] = useState<File | null>(null);
  const [backupConfirmOpen, setBackupConfirmOpen] = useState(false);
  const [backupBusy, setBackupBusy] = useState(false);

  const settingsQuery = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/api/settings'),
  });

  const draft =
    draftOverride ??
    (settingsQuery.data ? toDraft(settingsQuery.data) : EMPTY_DRAFT);

  function update(patch: Partial<SettingsDraft>) {
    setDraftOverride({ ...draft, ...patch });
  }

  const saveMutation = useMutation({
    mutationFn: (values: Settings) => api.put<Settings>('/api/settings', values),
    onSuccess: (data) => {
      toast.success('设置已保存');
      setDraftOverride(toDraft(data));
      queryClient.setQueryData(['settings'], data);
      queryClient.invalidateQueries({ queryKey: ['site'] });
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const checkUpdateMutation = useMutation({
    mutationFn: () => api.get<UpdateInfo>('/api/update/check'),
    onSuccess: (data) => {
      setUpdateInfo(data);
      setUpdateMessage(null);
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const applyUpdateMutation = useMutation({
    mutationFn: () => api.post<UpdateApplyResult>('/api/update/apply'),
    onSuccess: (data) => {
      setUpdateMessage(data.message);
      window.setTimeout(() => window.location.assign('/'), 3000);
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const passwordMutation = useMutation({
    mutationFn: (values: { current: string; next: string }) =>
      api.put('/api/auth/password', {
        current_password: values.current,
        new_password: values.next,
      }),
    onSuccess: () => {
      toast.success('密码已更新，其他会话已下线');
      setCurrentPassword('');
      setNewPassword('');
      setConfirmPassword('');
      setPasswordErrors({});
    },
    onError: (error) => {
      if (error instanceof ApiError && error.status === 401) {
        toast.error('当前密码不正确');
        return;
      }
      toast.error(errorMessage(error));
    },
  });

  function saveSettings() {
    if (!draft.site_name.trim()) {
      setSiteNameError('请输入站点名称');
      return;
    }
    setSiteNameError('');
    saveMutation.mutate(toSettings(draft));
  }

  function changePassword() {
    const errors: Record<string, string> = {};
    if (!currentPassword) {
      errors.current = '请输入当前密码';
    }
    if (!newPassword) {
      errors.next = '请输入新密码';
    } else if (newPassword.length < 8) {
      errors.next = '至少 8 个字符';
    }
    if (!confirmPassword) {
      errors.confirm = '请再次输入新密码';
    } else if (newPassword !== confirmPassword) {
      errors.confirm = '两次输入不一致';
    }
    setPasswordErrors(errors);
    if (Object.keys(errors).length > 0) {
      return;
    }
    passwordMutation.mutate({ current: currentPassword, next: newPassword });
  }

  async function downloadBackup() {
    setBackupBusy(true);
    try {
      const response = await requestRaw('/api/backup/export');
      const blob = await response.blob();
      const disposition = response.headers.get('content-disposition') ?? '';
      const filename =
        disposition.match(/filename="([^"]+)"/)?.[1] ?? 'oneapi-backup.db';
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = filename;
      document.body.appendChild(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
      toast.success('备份已下载');
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBackupBusy(false);
    }
  }

  async function importBackup() {
    if (!backupFile) {
      return;
    }
    setBackupBusy(true);
    try {
      const response = await requestRaw('/api/backup/import', {
        method: 'POST',
        headers: { 'Content-Type': 'application/octet-stream' },
        body: backupFile,
      });
      const value = (await response.json().catch(() => null)) as {
        message?: string;
      } | null;
      toast.success(value?.message ?? '导入成功，服务即将重启');
      setBackupConfirmOpen(false);
      setBackupFile(null);
    } catch (error) {
      toast.error(errorMessage(error));
    } finally {
      setBackupBusy(false);
    }
  }

  return (
    <>
      <PageHeader
        title="设置"
        description="站点参数、请求体记录、软件更新与管理员密码。"
      />

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle>站点设置</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {settingsQuery.isLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 8 }).map((_, index) => (
                  <Skeleton key={index} className="h-10 w-full" />
                ))}
              </div>
            ) : (
              <>
                <Field
                  label="站点名称"
                  htmlFor="settings-site-name"
                  error={siteNameError}
                >
                  <Input
                    id="settings-site-name"
                    value={draft.site_name}
                    onChange={(event) => {
                      update({ site_name: event.target.value });
                      setSiteNameError('');
                    }}
                  />
                </Field>

                <Field
                  label="网站域名"
                  htmlFor="settings-site-domain"
                  hint="反向代理后填写对外访问的域名，例如 https://api.example.com 或 api.example.com；协议、路径与默认端口会被忽略，仅支持 ASCII 域名或 IP。留空则只校验请求 Host。"
                >
                  <Input
                    id="settings-site-domain"
                    value={draft.site_domain}
                    placeholder="api.example.com"
                    className="font-mono"
                    onChange={(event) =>
                      update({ site_domain: event.target.value })
                    }
                  />
                </Field>

                <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
                  <Field
                    label="默认 max_tokens"
                    htmlFor="settings-max-tokens"
                    hint="请求未指定 max_tokens 时使用。"
                  >
                    <Input
                      id="settings-max-tokens"
                      type="number"
                      min={1}
                      value={draft.default_max_tokens}
                      onChange={(event) =>
                        update({ default_max_tokens: event.target.value })
                      }
                    />
                  </Field>
                  <Field
                    label="非流式超时（秒）"
                    htmlFor="settings-nonstream-timeout"
                  >
                    <Input
                      id="settings-nonstream-timeout"
                      type="number"
                      min={1}
                      value={draft.nonstream_timeout_seconds}
                      onChange={(event) =>
                        update({
                          nonstream_timeout_seconds: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    label="流式空闲超时（秒）"
                    htmlFor="settings-stream-timeout"
                  >
                    <Input
                      id="settings-stream-timeout"
                      type="number"
                      min={1}
                      value={draft.stream_idle_timeout_seconds}
                      onChange={(event) =>
                        update({
                          stream_idle_timeout_seconds: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    label="用量日志保留天数"
                    htmlFor="settings-usage-retention"
                    hint="0 表示永久保留。"
                  >
                    <Input
                      id="settings-usage-retention"
                      type="number"
                      min={0}
                      value={draft.usage_log_retention_days}
                      onChange={(event) =>
                        update({
                          usage_log_retention_days: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    label="Responses 保留天数"
                    htmlFor="settings-response-retention"
                    hint="0 表示永久保留。"
                  >
                    <Input
                      id="settings-response-retention"
                      type="number"
                      min={0}
                      value={draft.response_retention_days}
                      onChange={(event) =>
                        update({
                          response_retention_days: event.target.value,
                        })
                      }
                    />
                  </Field>
                </div>

                <div className="flex items-center justify-between rounded-lg border p-3">
                  <div className="pr-3">
                    <p className="text-sm font-medium">记录请求/响应体</p>
                    <p className="text-xs text-muted-foreground">
                      默认关闭；开启后单条最多保存 64KB，可在日志详情查看。
                    </p>
                  </div>
                  <Switch
                    checked={draft.store_request_bodies}
                    onCheckedChange={(checked) =>
                      update({ store_request_bodies: checked })
                    }
                  />
                </div>

                <Field
                  label="更新源"
                  hint="修改后请先保存设置，再检查更新。"
                >
                  <Select
                    items={UPDATE_SOURCE_OPTIONS}
                    value={draft.update_source}
                    onValueChange={(value) =>
                      update({
                        update_source: value as Settings['update_source'],
                      })
                    }
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="github">GitHub 官方</SelectItem>
                      <SelectItem value="ghproxy">
                        国内加速（gh-proxy.com）
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </Field>

                <Field
                  label="客户端 IP 请求头"
                  htmlFor="settings-client-ip"
                  hint="留空时默认读取 X-Forwarded-For（仅对回环/内网对端）；填写后视为可信并直接采用，适用于 CDN 从公网回源。请确保源站端口只接受 CDN/反向代理访问。"
                >
                  <Input
                    id="settings-client-ip"
                    list="client-ip-headers"
                    value={draft.client_ip_header}
                    onChange={(event) =>
                      update({ client_ip_header: event.target.value })
                    }
                    placeholder="例如 CF-Connecting-IP"
                    className="font-mono"
                  />
                  <datalist id="client-ip-headers">
                    <option value="cf-connecting-ip">
                      CF-Connecting-IP（Cloudflare）
                    </option>
                    <option value="x-forwarded-for">X-Forwarded-For</option>
                    <option value="x-real-ip">X-Real-IP</option>
                    <option value="true-client-ip">True-Client-IP</option>
                  </datalist>
                </Field>

                <Button
                  type="button"
                  onClick={saveSettings}
                  disabled={saveMutation.isPending}
                >
                  {saveMutation.isPending ? (
                    <Loader2 className="animate-spin" />
                  ) : null}
                  保存设置
                </Button>
              </>
            )}
          </CardContent>
        </Card>

        <Card className="self-start">
          <CardHeader>
            <CardTitle>修改密码</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            <Field
              label="当前密码"
              htmlFor="settings-current-password"
              error={passwordErrors.current}
            >
              <Input
                id="settings-current-password"
                type="password"
                autoComplete="current-password"
                value={currentPassword}
                onChange={(event) => {
                  setCurrentPassword(event.target.value);
                  setPasswordErrors((errors) => ({
                    ...errors,
                    current: '',
                  }));
                }}
              />
            </Field>
            <Field
              label="新密码"
              htmlFor="settings-new-password"
              error={passwordErrors.next}
              hint="至少 8 个字符。"
            >
              <Input
                id="settings-new-password"
                type="password"
                autoComplete="new-password"
                value={newPassword}
                onChange={(event) => {
                  setNewPassword(event.target.value);
                  setPasswordErrors((errors) => ({ ...errors, next: '' }));
                }}
              />
            </Field>
            <Field
              label="确认新密码"
              htmlFor="settings-confirm-password"
              error={passwordErrors.confirm}
            >
              <Input
                id="settings-confirm-password"
                type="password"
                autoComplete="new-password"
                value={confirmPassword}
                onChange={(event) => {
                  setConfirmPassword(event.target.value);
                  setPasswordErrors((errors) => ({
                    ...errors,
                    confirm: '',
                  }));
                }}
              />
            </Field>
            <Button
              type="button"
              onClick={changePassword}
              disabled={passwordMutation.isPending}
            >
              {passwordMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <ShieldCheck />
              )}
              更新密码
            </Button>
          </CardContent>
        </Card>
      </div>

      <Card className="mt-4">
        <CardHeader>
          <CardTitle>软件更新</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              onClick={() => checkUpdateMutation.mutate()}
              disabled={checkUpdateMutation.isPending}
            >
              {checkUpdateMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <RefreshCw />
              )}
              检查更新
            </Button>
            {updateInfo ? (
              <Badge
                variant="outline"
                className={
                  updateInfo.update_available
                    ? 'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300'
                    : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300'
                }
              >
                {updateInfo.update_available ? '发现新版本' : '已是最新版本'}
              </Badge>
            ) : null}
          </div>

          {updateInfo ? (
            <>
              <dl className="grid grid-cols-2 gap-x-4 gap-y-3 rounded-lg border p-3 sm:grid-cols-3">
                <div className="space-y-0.5">
                  <dt className="text-xs text-muted-foreground">当前版本</dt>
                  <dd className="font-mono text-sm">
                    {updateInfo.current_version}
                  </dd>
                </div>
                <div className="space-y-0.5">
                  <dt className="text-xs text-muted-foreground">最新版本</dt>
                  <dd className="font-mono text-sm">
                    {updateInfo.latest_version}
                  </dd>
                </div>
                <div className="space-y-0.5">
                  <dt className="text-xs text-muted-foreground">更新源</dt>
                  <dd className="text-sm">
                    {updateInfo.source === 'ghproxy'
                      ? 'gh-proxy.com 加速'
                      : 'GitHub 官方'}
                  </dd>
                </div>
                <div className="space-y-0.5">
                  <dt className="text-xs text-muted-foreground">发布时间</dt>
                  <dd className="text-sm">{updateInfo.published_at ?? '-'}</dd>
                </div>
                <div className="col-span-2 space-y-0.5">
                  <dt className="text-xs text-muted-foreground">安装包</dt>
                  <dd className="truncate font-mono text-sm">
                    {updateInfo.asset_name ?? '当前平台暂不支持'}
                  </dd>
                </div>
              </dl>

              {updateInfo.release_notes ? (
                <div className="space-y-2">
                  <h3 className="text-sm font-medium">更新说明</h3>
                  <CodeBlock>{updateInfo.release_notes}</CodeBlock>
                </div>
              ) : null}

              {updateInfo.update_available ? (
                <Button
                  variant="destructive"
                  onClick={() => applyUpdateMutation.mutate()}
                  disabled={
                    applyUpdateMutation.isPending || !updateInfo.asset_name
                  }
                >
                  {applyUpdateMutation.isPending ? (
                    <Loader2 className="animate-spin" />
                  ) : null}
                  一键更新并重启
                </Button>
              ) : null}
            </>
          ) : null}

          {updateMessage ? (
            <Alert>
              <AlertTitle>{updateMessage}</AlertTitle>
              <AlertDescription>
                服务会自动重启，页面将在几秒后刷新。
              </AlertDescription>
            </Alert>
          ) : null}

          <Alert>
            <AlertTitle>更新说明</AlertTitle>
            <AlertDescription>
              更新会下载 SHA256SUMS 校验安装包，替换当前二进制并自动重启。容器部署请拉取新镜像；Windows
              更新时需要保证程序目录可写。
            </AlertDescription>
          </Alert>
        </CardContent>
      </Card>

      <Card className="mt-4">
        <CardHeader>
          <CardTitle>数据库备份</CardTitle>
        </CardHeader>
        <CardContent className="space-y-4">
          <p className="text-sm text-muted-foreground">
            导出会下载一个一致的 SQLite 快照；导入会先校验文件，成功后自动重启并替换当前数据库。导入前当前数据库会保留在
            data/backups 目录。
          </p>
          <div>
            <Button
              variant="outline"
              onClick={downloadBackup}
              disabled={backupBusy}
            >
              {backupBusy ? <Loader2 className="animate-spin" /> : <Download />}
              导出备份
            </Button>
          </div>
          <div className="space-y-2">
            <Field
              label="恢复数据库"
              htmlFor="backup-import"
              hint="选择之前导出的 .db 文件；上传后会先校验，再自动重启。"
            >
              <Input
                id="backup-import"
                type="file"
                accept=".db,application/vnd.sqlite3"
                onChange={(event) =>
                  setBackupFile(event.target.files?.[0] ?? null)
                }
              />
            </Field>
            <Button
              variant="destructive"
              disabled={!backupFile || backupBusy}
              onClick={() => setBackupConfirmOpen(true)}
            >
              <Upload />
              导入并重启
            </Button>
          </div>
          <ConfirmDialog
            open={backupConfirmOpen}
            onOpenChange={setBackupConfirmOpen}
            title="用备份替换当前数据库？"
            description="当前数据库会先自动备份到 data/backups，然后服务重启并加载上传的文件。重启期间请求会短暂失败。"
            confirmLabel="导入并重启"
            loading={backupBusy}
            destructive
            onConfirm={importBackup}
          />
        </CardContent>
      </Card>
    </>
  );
}
