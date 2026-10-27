import { Alert, Button, Card, Form, Input, Typography } from 'antd';
import { useMutation } from '@tanstack/react-query';
import { api } from '../api/client';

interface LoginValues {
  username: string;
  password: string;
}

export default function Login({ onSuccess }: { onSuccess: () => void }) {
  const login = useMutation({
    mutationFn: (values: LoginValues) =>
      api.post<{ username: string }>('/api/auth/login', values),
    onSuccess,
  });

  return (
    <div className="login-shell">
      <Card style={{ width: 380 }}>
        <Typography.Title level={3} style={{ marginBottom: 4 }}>
          OneAPI
        </Typography.Title>
        <Typography.Paragraph type="secondary">登录管理后台</Typography.Paragraph>
        {login.isError && (
          <Alert
            type="error"
            showIcon
            message={login.error instanceof Error ? login.error.message : '登录失败'}
            style={{ marginBottom: 16 }}
          />
        )}
        <Form layout="vertical" onFinish={(values: LoginValues) => login.mutate(values)}>
          <Form.Item
            name="username"
            label="用户名"
            rules={[{ required: true, message: '请输入用户名' }]}
          >
            <Input autoComplete="username" />
          </Form.Item>
          <Form.Item
            name="password"
            label="密码"
            rules={[{ required: true, message: '请输入密码' }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Button type="primary" htmlType="submit" block loading={login.isPending}>
            登录
          </Button>
        </Form>
      </Card>
    </div>
  );
}
