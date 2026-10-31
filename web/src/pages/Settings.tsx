import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  AlertCircle,
  Archive,
  CheckCircle2,
  Loader2,
  RefreshCw,
  TriangleAlert,
} from 'lucide-react';
import { useEffect, useState } from 'react';
import { useLocation } from 'react-router-dom';
import { toast } from 'sonner';
import { api, ApiError } from '@/api/client';
import type {
  BackupListOutput,
  BackupSnapshotResult,
  Settings,
  UpdateApplyResult,
  UpdateInfo,
} from '@/api/types';
import { CodeBlock } from '@/components/common/code-block';
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
import { refreshUpdateCheck } from '@/hooks/useUpdateCheck';
import { errorMessage } from '@/lib/error';
import { formatBytes, formatDateTime } from '@/lib/format';

interface SettingsDraft {
  site_name: string;
  site_domain: string;
  default_max_tokens: string;
  nonstream_timeout_seconds: string;
  stream_idle_timeout_seconds: string;
  usage_log_retention_days: string;
  request_detail_retention_days: string;
  response_retention_days: string;
  response_storage_per_key_mb: string;
  response_storage_total_mb: string;
  store_request_bodies: boolean;
  update_source: 'github' | 'ghproxy';
  client_ip_header: string;
}

const UPDATE_SOURCE_OPTIONS = [
  { value: 'github', label: 'GitHub 官方' },
  { value: 'ghproxy', label: '国内加速（gh-proxy.com）' },
];

interface UpdateFailure {
  message: string;
  hint: string;
  severity: 'warning' | 'error';
}

/**
 * Map an update API failure to the presentation it deserves. The backend
 * classifies recoverable transport/metadata problems as `warning`; anything
 * that failed while installing is `error`.
 */
function updateFailureOf(error: unknown): UpdateFailure {
  const details = error instanceof ApiError ? error.details : undefined;
  return {
    message: errorMessage(error),
    hint: details?.hint ?? '请查看服务端日志定位原因后重试。',
    severity: details?.severity === 'warning' ? 'warning' : 'error',
  };
}

/// Numeric defaults. Used both to seed an empty draft and as the fallback when
/// a field is cleared, so the two can never disagree.
const NUMERIC_DEFAULTS = {
  default_max_tokens: 4096,
  nonstream_timeout_seconds: 600,
  stream_idle_timeout_seconds: 120,
  usage_log_retention_days: 30,
  request_detail_retention_days: 7,
  response_retention_days: 7,
  response_storage_per_key_mb: 64,
  response_storage_total_mb: 2048,
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
  request_detail_retention_days: String(
    NUMERIC_DEFAULTS.request_detail_retention_days,
  ),
  response_retention_days: String(NUMERIC_DEFAULTS.response_retention_days),
  response_storage_per_key_mb: String(
    NUMERIC_DEFAULTS.response_storage_per_key_mb,
  ),
  response_storage_total_mb: String(
    NUMERIC_DEFAULTS.response_storage_total_mb,
  ),
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
    request_detail_retention_days: String(
      settings.request_detail_retention_days,
    ),
    response_retention_days: String(settings.response_retention_days),
    response_storage_per_key_mb: String(settings.response_storage_per_key_mb),
    response_storage_total_mb: String(settings.response_storage_total_mb),
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
    request_detail_retention_days: toInt(
      draft.request_detail_retention_days,
      NUMERIC_DEFAULTS.request_detail_retention_days,
    ),
    response_retention_days: toInt(
      draft.response_retention_days,
      NUMERIC_DEFAULTS.response_retention_days,
    ),
    response_storage_per_key_mb: toInt(
      draft.response_storage_per_key_mb,
      NUMERIC_DEFAULTS.response_storage_per_key_mb,
    ),
    response_storage_total_mb: toInt(
      draft.response_storage_total_mb,
      NUMERIC_DEFAULTS.response_storage_total_mb,
    ),
    store_request_bodies: draft.store_request_bodies,
    update_source: draft.update_source,
    client_ip_header: draft.client_ip_header.trim(),
  };
}

/**
 * Only the fields the user actually edited are sent. The backend treats every
 * field as optional, so this avoids writing defaults back over settings that
 * were never loaded or never touched.
 */
function toSettingsPatch(
  draft: SettingsDraft,
  fields: ReadonlySet<keyof SettingsDraft>,
): Partial<Settings> {
  const values = toSettings(draft);
  return Object.fromEntries(
    Array.from(fields, (field) => [field, values[field]]),
  ) as Partial<Settings>;
}

export default function SettingsPage() {
  const queryClient = useQueryClient();
  const location = useLocation();
  const [draftOverride, setDraftOverride] = useState<SettingsDraft | null>(
    null,
  );
  // Fields the user edited while a server copy exists. Only these are sent on
  // save, so unrelated values are never replaced with local defaults.
  const [dirtyFields, setDirtyFields] = useState<Set<keyof SettingsDraft>>(
    () => new Set(),
  );
  const [siteNameError, setSiteNameError] = useState('');
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateResult, setUpdateResult] = useState<UpdateApplyResult | null>(
    null,
  );
  const [updateFailure, setUpdateFailure] = useState<UpdateFailure | null>(
    null,
  );

  const settingsQuery = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/api/settings'),
  });

  function reportUpdateFailure(error: unknown) {
    const failure = updateFailureOf(error);
    setUpdateResult(null);
    setUpdateFailure(failure);
    const options = { description: failure.hint, duration: 10000 };
    if (failure.severity === 'warning') {
      toast.warning(failure.message, options);
    } else {
      toast.error(failure.message, options);
    }
  }

  const backupsQuery = useQuery({
    queryKey: ['backups'],
    queryFn: () => api.get<BackupListOutput>('/api/backup/list'),
  });

  const snapshotMutation = useMutation({
    mutationFn: () => api.post<BackupSnapshotResult>('/api/backup/snapshot'),
    onSuccess: (data) => {
      toast.success(`${data.message}：${data.file}`);
      void queryClient.invalidateQueries({ queryKey: ['backups'] });
      void queryClient.invalidateQueries({ queryKey: ['system'] });
    },
    onError: (error) => toast.error(errorMessage(error)),
  });

  const draft =
    draftOverride ??
    (settingsQuery.data ? toDraft(settingsQuery.data) : EMPTY_DRAFT);

  function update(patch: Partial<SettingsDraft>) {
    setDraftOverride({ ...draft, ...patch });
    setDirtyFields((current) => {
      const next = new Set(current);
      for (const key of Object.keys(patch) as (keyof SettingsDraft)[]) {
        next.add(key);
      }
      return next;
    });
  }

  const saveMutation = useMutation({
    mutationFn: (values: Partial<Settings>) =>
      api.put<Settings>('/api/settings', values),
    onSuccess: (data) => {
      toast.success('设置已保存');
      setDraftOverride(toDraft(data));
      setDirtyFields(new Set());
      queryClient.setQueryData(['settings'], data);
      queryClient.invalidateQueries({ queryKey: ['site'] });
    },
    onError: (error) =>
      toast.error(errorMessage(error)),
  });

  const checkUpdateMutation = useMutation({
    mutationFn: () => refreshUpdateCheck(queryClient),
    onSuccess: (data) => {
      setUpdateInfo(data);
      setUpdateResult(null);
      setUpdateFailure(null);
    },
    onError: (error) => reportUpdateFailure(error),
  });

  const applyUpdateMutation = useMutation({
    mutationFn: () => api.post<UpdateApplyResult>('/api/update/apply'),
    onSuccess: (data) => {
      setUpdateResult(data);
      setUpdateFailure(null);
      toast.success('更新成功', {
        description: '服务正在重启，页面将在几秒后自动刷新。',
      });
      window.setTimeout(() => window.location.assign('/'), 3000);
    },
    onError: (error) => reportUpdateFailure(error),
  });

  // The version-badge red dot links here with `#update`; the sticky header
  // needs `scroll-mt` so the card title is not hidden behind it.
  useEffect(() => {
    if (location.hash !== '#update') {
      return;
    }
    document
      .getElementById('software-update')
      ?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  }, [location.hash]);

  function saveSettings() {
    // Without server data the form must not be usable; saving here would write
    // EMPTY_DRAFT defaults over the stored configuration.
    if (!settingsQuery.data) {
      toast.error('设置尚未加载，无法保存');
      return;
    }
    if (dirtyFields.has('site_name') && !draft.site_name.trim()) {
      setSiteNameError('请输入站点名称');
      return;
    }
    setSiteNameError('');
    saveMutation.mutate(toSettingsPatch(draft, dirtyFields));
  }

  return (
    <>
      <PageHeader
        title="设置"
        description="站点参数、请求体记录与软件更新。"
      />

      <div className="grid gap-4">
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
            ) : settingsQuery.isError ? (
              <Alert variant="destructive">
                <AlertTitle>加载设置失败</AlertTitle>
                <AlertDescription>
                  {settingsQuery.error instanceof Error
                    ? settingsQuery.error.message
                    : String(settingsQuery.error)}
                </AlertDescription>
                <div className="mt-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => void settingsQuery.refetch()}
                    disabled={settingsQuery.isFetching}
                  >
                    {settingsQuery.isFetching ? (
                      <Loader2 className="animate-spin" />
                    ) : (
                      <RefreshCw />
                    )}
                    重试
                  </Button>
                </div>
              </Alert>
            ) : settingsQuery.data ? (
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
                    label="请求正文保留天数"
                    htmlFor="settings-request-detail-retention"
                    hint="请求/响应正文独立于用量日志保留；0 表示不按时间清理，仅在删除 Key 时清除。"
                  >
                    <Input
                      id="settings-request-detail-retention"
                      type="number"
                      min={0}
                      max={3650}
                      value={draft.request_detail_retention_days}
                      onChange={(event) =>
                        update({
                          request_detail_retention_days: event.target.value,
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
                  <Field
                    label="Responses 单 Key 存储上限（MB）"
                    htmlFor="settings-response-per-key-mb"
                    hint="单个 API Key 的 Responses 存储上限；0 表示不限制，超额返回 store: false。"
                  >
                    <Input
                      id="settings-response-per-key-mb"
                      type="number"
                      min={0}
                      max={1048576}
                      value={draft.response_storage_per_key_mb}
                      onChange={(event) =>
                        update({
                          response_storage_per_key_mb: event.target.value,
                        })
                      }
                    />
                  </Field>
                  <Field
                    label="Responses 全局存储上限（MB）"
                    htmlFor="settings-response-total-mb"
                    hint="所有 API Key 合计的 Responses 存储上限；0 表示不限制。"
                  >
                    <Input
                      id="settings-response-total-mb"
                      type="number"
                      min={0}
                      max={1048576}
                      value={draft.response_storage_total_mb}
                      onChange={(event) =>
                        update({
                          response_storage_total_mb: event.target.value,
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
            ) : null}
          </CardContent>
        </Card>
      </div>

      <Card id="software-update" className="mt-4 scroll-mt-20">
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
                  variant={updateResult ? 'default' : 'destructive'}
                  className={
                    updateResult
                      ? 'bg-emerald-600 text-white hover:bg-emerald-600/90'
                      : undefined
                  }
                  onClick={() => applyUpdateMutation.mutate()}
                  disabled={
                    applyUpdateMutation.isPending ||
                    updateResult != null ||
                    !updateInfo.asset_name
                  }
                >
                  {applyUpdateMutation.isPending ? (
                    <Loader2 className="animate-spin" />
                  ) : updateResult ? (
                    <CheckCircle2 />
                  ) : null}
                  {applyUpdateMutation.isPending
                    ? '正在更新…'
                    : updateResult
                      ? '更新成功'
                      : '一键更新并重启'}
                </Button>
              ) : null}
            </>
          ) : null}

          {updateResult ? (
            <Alert variant="success">
              <CheckCircle2 />
              <AlertTitle>更新成功</AlertTitle>
              <AlertDescription>
                已更新到 {updateResult.updated_to}。
                {updateResult.message}，页面将在几秒后自动刷新。
              </AlertDescription>
            </Alert>
          ) : null}

          {updateFailure ? (
            <Alert
              variant={
                updateFailure.severity === 'warning' ? 'warning' : 'destructive'
              }
            >
              {updateFailure.severity === 'warning' ? (
                <TriangleAlert />
              ) : (
                <AlertCircle />
              )}
              <AlertTitle>
                {updateFailure.severity === 'warning'
                  ? '更新未完成，可尝试修复后重试'
                  : '更新失败，需要手动排查'}
              </AlertTitle>
              <AlertDescription>
                <p>{updateFailure.message}</p>
                <p className="mt-1">{updateFailure.hint}</p>
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
            备份只在服务器端生成，保存在 data/backups
            目录，保留最近 5 份；不提供下载或上传，避免整库被导出或被上传文件替换。
            需要恢复时请在服务器上停止服务，用目标备份覆盖数据库文件并删除对应的
            -wal/-shm。
          </p>
          <div className="flex flex-wrap items-center gap-3">
            <Button
              variant="outline"
              onClick={() => snapshotMutation.mutate()}
              disabled={snapshotMutation.isPending}
            >
              {snapshotMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <Archive />
              )}
              立即备份到服务器
            </Button>
            <Button
              variant="ghost"
              onClick={() => void backupsQuery.refetch()}
              disabled={backupsQuery.isFetching}
            >
              {backupsQuery.isFetching ? (
                <Loader2 className="animate-spin" />
              ) : (
                <RefreshCw />
              )}
              刷新
            </Button>
          </div>
          {backupsQuery.data?.items.length ? (
            <ul className="divide-y rounded-md border">
              {backupsQuery.data.items.map((file) => (
                <li
                  key={file.name}
                  className="flex flex-wrap items-center justify-between gap-2 px-3 py-2 text-sm"
                >
                  <span className="truncate font-mono text-xs">{file.name}</span>
                  <span className="text-xs text-muted-foreground">
                    {formatBytes(file.bytes)} ·{' '}
                    {formatDateTime(file.modified_at, 'YYYY-MM-DD HH:mm')}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-sm text-muted-foreground">暂无备份文件。</p>
          )}
        </CardContent>
      </Card>
    </>
  );
}
