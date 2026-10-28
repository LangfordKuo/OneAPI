import { useMutation } from '@tanstack/react-query';
import {
  AlertCircle,
  AlertTriangle,
  Boxes,
  CheckCircle2,
  Database,
  FileCog,
  FolderOpen,
  Loader2,
  Server,
} from 'lucide-react';
import { useState } from 'react';
import { api } from '@/api/client';
import type { SetupResult, SetupStatus } from '@/api/types';
import { Field } from '@/components/common/field';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Input } from '@/components/ui/input';

interface Props {
  status: SetupStatus;
  siteName?: string;
  onSuccess: () => void;
}

interface SetupFormValues {
  username: string;
  password: string;
  confirm: string;
  site_name: string;
}

function StatusRow({
  icon: Icon,
  label,
  value,
}: {
  icon: typeof Server;
  label: string;
  value?: string;
}) {
  return (
    <div className="flex items-start gap-2.5 rounded-lg border bg-muted/30 px-3 py-2.5">
      <Icon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
      <div className="min-w-0">
        <div className="text-[11px] text-muted-foreground">{label}</div>
        <div className="truncate font-mono text-xs" title={value}>
          {value ?? '-'}
        </div>
      </div>
    </div>
  );
}

export default function Setup({ status, siteName = 'OneAPI', onSuccess }: Props) {
  const [result, setResult] = useState<SetupResult | null>(null);
  const [values, setValues] = useState<SetupFormValues>({
    username: 'admin',
    password: '',
    confirm: '',
    site_name: siteName,
  });
  const [errors, setErrors] = useState<Record<string, string>>({});

  const mutation = useMutation({
    mutationFn: (payload: SetupFormValues) =>
      api.post<SetupResult>('/api/setup', {
        username: payload.username,
        password: payload.password,
        site_name: payload.site_name,
      }),
    onSuccess: (data) => setResult(data),
  });

  function update<K extends keyof SetupFormValues>(
    key: K,
    value: SetupFormValues[K],
  ) {
    setValues((previous) => ({ ...previous, [key]: value }));
    setErrors((previous) => ({ ...previous, [key]: '' }));
  }

  function submit() {
    const nextErrors: Record<string, string> = {};
    if (!values.username.trim()) {
      nextErrors.username = '请输入管理员用户名';
    }
    if (values.password.length < 8) {
      nextErrors.password = '密码至少 8 位';
    }
    if (values.confirm !== values.password) {
      nextErrors.confirm = '两次输入的密码不一致';
    }
    setErrors(nextErrors);
    if (Object.keys(nextErrors).length > 0) {
      return;
    }
    mutation.mutate(values);
  }

  if (result) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-muted/30 p-6">
        <Card className="w-full max-w-2xl">
          <CardHeader className="items-center justify-items-center text-center">
            <div className="mb-2 flex size-12 items-center justify-center rounded-full bg-emerald-500/10 text-emerald-600 dark:text-emerald-400">
              <CheckCircle2 className="size-6" />
            </div>
            <CardTitle className="text-xl">安装完成</CardTitle>
            <CardDescription>
              管理员 {result.username} 已创建，SQLite 数据库已就绪。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="grid gap-2 sm:grid-cols-2">
              <StatusRow
                icon={FileCog}
                label="配置文件"
                value={result.env_path}
              />
              <StatusRow
                icon={FolderOpen}
                label="数据目录"
                value={status.data_dir}
              />
            </div>

            {!result.env_written ? (
              <Alert>
                <AlertTriangle />
                <AlertTitle>.env 未写入</AlertTitle>
                <AlertDescription>{result.message}</AlertDescription>
              </Alert>
            ) : null}

            <Button className="w-full" size="lg" onClick={onSuccess}>
              进入管理后台
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-muted/30 p-6">
      <Card className="w-full max-w-2xl">
        <CardHeader>
          <div className="mb-2 flex items-center gap-2.5">
            <div className="flex size-9 items-center justify-center rounded-lg bg-primary text-primary-foreground">
              <Boxes className="size-4" />
            </div>
            <span className="text-sm font-semibold">{siteName}</span>
          </div>
          <CardTitle className="text-xl">首次安装</CardTitle>
          <CardDescription>
            创建管理员账号后，OneAPI 会写入 .env 并自动登录。SQLite
            数据库已在启动时创建。
          </CardDescription>
        </CardHeader>

        <CardContent className="space-y-5">
          <div className="grid gap-2 sm:grid-cols-2">
            <StatusRow icon={Server} label="版本" value={status.version} />
            <StatusRow icon={Server} label="监听地址" value={status.addr} />
            <StatusRow
              icon={FolderOpen}
              label="数据目录"
              value={status.data_dir}
            />
            <StatusRow icon={Database} label="SQLite" value={status.database} />
            <div className="sm:col-span-2">
              <StatusRow
                icon={FileCog}
                label="配置文件"
                value={status.env_path}
              />
            </div>
          </div>

          <form
            className="space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              submit();
            }}
          >
            <div className="grid gap-4 sm:grid-cols-2">
              <Field
                label="管理员用户名"
                htmlFor="setup-username"
                error={errors.username}
              >
                <Input
                  id="setup-username"
                  autoComplete="username"
                  value={values.username}
                  onChange={(event) => update('username', event.target.value)}
                />
              </Field>
              <Field label="站点名称" htmlFor="setup-site-name">
                <Input
                  id="setup-site-name"
                  value={values.site_name}
                  onChange={(event) => update('site_name', event.target.value)}
                />
              </Field>
            </div>

            <div className="grid gap-4 sm:grid-cols-2">
              <Field
                label="管理员密码"
                htmlFor="setup-password"
                hint="至少 8 位"
                error={errors.password}
              >
                <Input
                  id="setup-password"
                  type="password"
                  autoComplete="new-password"
                  value={values.password}
                  onChange={(event) => update('password', event.target.value)}
                />
              </Field>
              <Field
                label="确认密码"
                htmlFor="setup-confirm"
                error={errors.confirm}
              >
                <Input
                  id="setup-confirm"
                  type="password"
                  autoComplete="new-password"
                  value={values.confirm}
                  onChange={(event) => update('confirm', event.target.value)}
                />
              </Field>
            </div>

            {mutation.isError ? (
              <Alert variant="destructive">
                <AlertCircle />
                <AlertTitle>安装失败</AlertTitle>
                <AlertDescription>
                  {mutation.error instanceof Error
                    ? mutation.error.message
                    : String(mutation.error)}
                </AlertDescription>
              </Alert>
            ) : null}

            <Button
              type="submit"
              size="lg"
              className="w-full"
              disabled={mutation.isPending}
            >
              {mutation.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : null}
              完成安装
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
