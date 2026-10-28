import { useMutation } from '@tanstack/react-query';
import {
  AlertCircle,
  Boxes,
  Loader2,
  ShieldCheck,
  Waypoints,
  Zap,
} from 'lucide-react';
import { useState } from 'react';
import { api } from '@/api/client';
import { Field } from '@/components/common/field';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

interface LoginProps {
  onSuccess: () => void;
  siteName?: string;
  version?: string;
}

export default function Login({
  onSuccess,
  siteName = 'OneAPI',
  version,
}: LoginProps) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');

  const login = useMutation({
    mutationFn: (values: { username: string; password: string }) =>
      api.post<{ username: string }>('/api/auth/login', values),
    onSuccess,
  });

  return (
    <div className="grid min-h-screen lg:grid-cols-2">
      <div className="relative hidden overflow-hidden bg-zinc-950 p-12 text-white lg:flex lg:flex-col lg:justify-between">
        <div className="absolute inset-0 bg-[linear-gradient(to_right,rgba(255,255,255,0.05)_1px,transparent_1px),linear-gradient(to_bottom,rgba(255,255,255,0.05)_1px,transparent_1px)] bg-[size:36px_36px]" />
        <div className="absolute -left-24 -top-24 size-96 rounded-full bg-blue-500/20 blur-3xl" />
        <div className="absolute -bottom-32 right-0 size-96 rounded-full bg-emerald-500/10 blur-3xl" />

        <div className="relative flex items-center gap-3">
          <div className="flex size-10 items-center justify-center rounded-xl bg-white/10 ring-1 ring-white/15">
            <Boxes className="size-5" />
          </div>
          <div>
            <div className="text-base font-semibold">{siteName}</div>
            <div className="text-xs text-white/50">AI API Gateway</div>
          </div>
        </div>

        <div className="relative max-w-md space-y-6">
          <h2 className="text-3xl font-semibold leading-tight tracking-tight">
            一个入口，统一接入
            <br />
            三种主流 AI 协议
          </h2>
          <p className="text-sm leading-relaxed text-white/60">
            Chat Completions、Responses 与 Anthropic Messages 双向转换，
            多渠道按优先级与权重路由，失败自动切换。
          </p>
          <div className="space-y-3 text-sm text-white/70">
            <div className="flex items-center gap-3">
              <Waypoints className="size-4 text-blue-300" />
              跨协议转换与同协议透传
            </div>
            <div className="flex items-center gap-3">
              <Zap className="size-4 text-amber-300" />
              流式响应、实时 tok/s 与用量日志
            </div>
            <div className="flex items-center gap-3">
              <ShieldCheck className="size-4 text-emerald-300" />
              本地 SQLite、密钥加密与自托管部署
            </div>
          </div>
        </div>

        <div className="relative text-xs text-white/40">
          {version ? `Version ${version}` : 'Self-hosted'}
        </div>
      </div>

      <div className="flex items-center justify-center p-6">
        <div className="w-full max-w-sm">
          <div className="mb-8 flex items-center gap-2.5 lg:hidden">
            <div className="flex size-9 items-center justify-center rounded-lg bg-primary text-primary-foreground">
              <Boxes className="size-4" />
            </div>
            <div>
              <div className="text-sm font-semibold">{siteName}</div>
              <div className="text-[11px] text-muted-foreground">
                AI API Gateway
              </div>
            </div>
          </div>

          <div className="space-y-1.5">
            <h1 className="text-2xl font-semibold tracking-tight">
              登录管理后台
            </h1>
            <p className="text-sm text-muted-foreground">
              使用管理员账号继续。
            </p>
          </div>

          <form
            className="mt-6 space-y-4"
            onSubmit={(event) => {
              event.preventDefault();
              login.mutate({ username, password });
            }}
          >
            <Field label="用户名" htmlFor="username">
              <Input
                id="username"
                autoComplete="username"
                value={username}
                onChange={(event) => setUsername(event.target.value)}
                placeholder="admin"
                required
              />
            </Field>

            <Field label="密码" htmlFor="password">
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                placeholder="••••••••"
                required
              />
            </Field>

            {login.isError ? (
              <Alert variant="destructive">
                <AlertCircle />
                <AlertTitle>登录失败</AlertTitle>
                <AlertDescription>
                  {login.error instanceof Error
                    ? login.error.message
                    : '请检查用户名和密码'}
                </AlertDescription>
              </Alert>
            ) : null}

            <Button
              type="submit"
              size="lg"
              className="w-full"
              disabled={login.isPending || !username || !password}
            >
              {login.isPending ? (
                <Loader2 className="size-4 animate-spin" />
              ) : null}
              登录
            </Button>
          </form>

          <p className="mt-6 text-center text-xs text-muted-foreground">
            登录状态会保存在 HttpOnly Cookie 中，7 天后过期。
          </p>
        </div>
      </div>
    </div>
  );
}
