import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Loader2, Save, ShieldCheck } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { api, ApiError } from '@/api/client';
import type { Admin } from '@/api/types';
import { Field } from '@/components/common/field';
import { PageHeader } from '@/components/common/page-header';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import { errorMessage } from '@/lib/error';

const MAX_USERNAME_CHARS = 64;

/** Shared 401 wording for both forms: the API only reports "unauthorized". */
function reportCredentialError(error: unknown): void {
  if (error instanceof ApiError && error.status === 401) {
    toast.error('当前密码不正确');
    return;
  }
  toast.error(errorMessage(error));
}

export default function Account() {
  const queryClient = useQueryClient();
  const meQuery = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Admin>('/api/auth/me'),
  });
  const currentUsername = meQuery.data?.username ?? '';

  const [username, setUsername] = useState('');
  const [usernamePassword, setUsernamePassword] = useState('');
  const [usernameErrors, setUsernameErrors] = useState<Record<string, string>>(
    {},
  );

  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [passwordErrors, setPasswordErrors] = useState<Record<string, string>>(
    {},
  );

  const usernameMutation = useMutation({
    mutationFn: (values: { username: string; current: string }) =>
      api.put<Admin>('/api/auth/account', {
        username: values.username,
        current_password: values.current,
      }),
    onSuccess: (admin) => {
      toast.success('用户名已更新');
      queryClient.setQueryData(['me'], admin);
      setUsername('');
      setUsernamePassword('');
      setUsernameErrors({});
    },
    onError: (error) => reportCredentialError(error),
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
    onError: (error) => reportCredentialError(error),
  });

  function saveUsername() {
    const errors: Record<string, string> = {};
    const next = username.trim();
    if (!next) {
      errors.username = '请输入新的用户名';
    } else if (next === currentUsername) {
      errors.username = '新用户名不能与当前用户名相同';
    } else if (next.length > MAX_USERNAME_CHARS) {
      errors.username = `最多 ${MAX_USERNAME_CHARS} 个字符`;
    } else if (
      Array.from(next).some((char) => {
        const code = char.charCodeAt(0);
        return code < 0x20 || code === 0x7f;
      })
    ) {
      errors.username = '不能包含控制字符';
    }
    if (!usernamePassword) {
      errors.current = '请输入当前密码';
    }
    setUsernameErrors(errors);
    if (Object.keys(errors).length > 0) {
      return;
    }
    usernameMutation.mutate({ username: next, current: usernamePassword });
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

  return (
    <>
      <PageHeader
        title="账户设置"
        description="管理管理员用户名与登录密码；修改密码后其他会话会立即下线。"
      />

      <div className="grid gap-4 lg:grid-cols-2">
        <Card className="self-start">
          <CardHeader>
            <CardTitle>修改用户名</CardTitle>
            <CardDescription>
              当前用户名：{currentUsername || '—'}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Field
              label="新用户名"
              htmlFor="account-username"
              error={usernameErrors.username}
              hint={`最多 ${MAX_USERNAME_CHARS} 个字符。`}
            >
              <Input
                id="account-username"
                autoComplete="username"
                value={username}
                placeholder={currentUsername}
                onChange={(event) => {
                  setUsername(event.target.value);
                  setUsernameErrors((errors) => ({ ...errors, username: '' }));
                }}
              />
            </Field>
            <Field
              label="当前密码"
              htmlFor="account-username-password"
              error={usernameErrors.current}
              hint="修改用户名需要验证当前密码。"
            >
              <Input
                id="account-username-password"
                type="password"
                autoComplete="current-password"
                value={usernamePassword}
                onChange={(event) => {
                  setUsernamePassword(event.target.value);
                  setUsernameErrors((errors) => ({ ...errors, current: '' }));
                }}
              />
            </Field>
            <Button
              type="button"
              onClick={saveUsername}
              disabled={usernameMutation.isPending}
            >
              {usernameMutation.isPending ? (
                <Loader2 className="animate-spin" />
              ) : (
                <Save />
              )}
              保存用户名
            </Button>
          </CardContent>
        </Card>

        <Card className="self-start">
          <CardHeader>
            <CardTitle>修改密码</CardTitle>
            <CardDescription>
              更新后其他设备上的会话会被强制下线。
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Field
              label="当前密码"
              htmlFor="account-current-password"
              error={passwordErrors.current}
            >
              <Input
                id="account-current-password"
                type="password"
                autoComplete="current-password"
                value={currentPassword}
                onChange={(event) => {
                  setCurrentPassword(event.target.value);
                  setPasswordErrors((errors) => ({ ...errors, current: '' }));
                }}
              />
            </Field>
            <Field
              label="新密码"
              htmlFor="account-new-password"
              error={passwordErrors.next}
              hint="至少 8 个字符。"
            >
              <Input
                id="account-new-password"
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
              htmlFor="account-confirm-password"
              error={passwordErrors.confirm}
            >
              <Input
                id="account-confirm-password"
                type="password"
                autoComplete="new-password"
                value={confirmPassword}
                onChange={(event) => {
                  setConfirmPassword(event.target.value);
                  setPasswordErrors((errors) => ({ ...errors, confirm: '' }));
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
    </>
  );
}
