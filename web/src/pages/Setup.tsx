import { useState } from 'react';
import { useMutation } from '@tanstack/react-query';
import {
  Alert,
  Button,
  Card,
  Descriptions,
  Form,
  Input,
  Result,
  Typography,
} from 'antd';
import { api } from '../api/client';
import type { SetupResult, SetupStatus } from '../api/types';

interface Props {
  status: SetupStatus;
  onSuccess: () => void;
}

interface SetupFormValues {
  username: string;
  password: string;
  confirm: string;
  site_name?: string;
}

export default function Setup({ status, onSuccess }: Props) {
  const [result, setResult] = useState<SetupResult | null>(null);

  const mutation = useMutation({
    mutationFn: (values: SetupFormValues) =>
      api.post<SetupResult>('/api/setup', {
        username: values.username,
        password: values.password,
        site_name: values.site_name,
      }),
    onSuccess: (data) => setResult(data),
  });

  if (result) {
    return (
      <div className="login-shell">
        <Card style={{ width: 620 }}>
          <Result
            status="success"
            title="安装完成"
            subTitle={`管理员 ${result.username} 已创建，SQLite 数据库已就绪。`}
          />
          <Descriptions column={1} size="small" bordered>
            <Descriptions.Item label="配置文件">{result.env_path}</Descriptions.Item>
            <Descriptions.Item label="数据库目录">{status.data_dir}</Descriptions.Item>
            <Descriptions.Item label="说明">{result.message}</Descriptions.Item>
          </Descriptions>
          {!result.env_written ? (
            <Alert
              type="warning"
              showIcon
              message=".env 未写入"
              description={result.message}
              style={{ marginTop: 16 }}
            />
          ) : null}
          <Button type="primary" block style={{ marginTop: 16 }} onClick={onSuccess}>
            进入后台
          </Button>
        </Card>
      </div>
    );
  }

  return (
    <div className="login-shell">
      <Card style={{ width: 680 }}>
        <Typography.Title level={3} style={{ marginTop: 0 }}>
          OneAPI 首次安装
        </Typography.Title>
        <Alert
          type="info"
          showIcon
          message="首次运行"
          description="创建管理员账号后，OneAPI 会写入 .env 并自动登录。SQLite 数据库已在启动时创建。监听地址和数据目录由启动环境变量或默认值决定。"
          style={{ marginBottom: 16 }}
        />

        <Descriptions column={1} size="small" bordered style={{ marginBottom: 16 }}>
          <Descriptions.Item label="版本">{status.version}</Descriptions.Item>
          <Descriptions.Item label="监听地址">{status.addr ?? '-'}</Descriptions.Item>
          <Descriptions.Item label="数据目录">{status.data_dir ?? '-'}</Descriptions.Item>
          <Descriptions.Item label="SQLite">{status.database ?? '-'}</Descriptions.Item>
          <Descriptions.Item label="配置文件">{status.env_path ?? '-'}</Descriptions.Item>
        </Descriptions>

        <Form<SetupFormValues>
          layout="vertical"
          initialValues={{ username: 'admin', site_name: 'OneAPI' }}
          onFinish={(values) => mutation.mutate(values)}
        >
          <Form.Item
            name="username"
            label="管理员用户名"
            rules={[{ required: true, whitespace: true, message: '请输入管理员用户名' }]}
          >
            <Input autoComplete="username" />
          </Form.Item>
          <Form.Item
            name="password"
            label="管理员密码"
            rules={[
              { required: true, message: '请输入管理员密码' },
              { min: 8, message: '密码至少 8 位' },
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm"
            label="确认密码"
            dependencies={['password']}
            rules={[
              { required: true, message: '请再次输入密码' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error('两次输入的密码不一致'));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item name="site_name" label="站点名称">
            <Input />
          </Form.Item>

          {mutation.error ? (
            <Alert
              type="error"
              showIcon
              message="安装失败"
              description={
                mutation.error instanceof Error
                  ? mutation.error.message
                  : String(mutation.error)
              }
              style={{ marginBottom: 16 }}
            />
          ) : null}

          <Button type="primary" htmlType="submit" block loading={mutation.isPending}>
            完成安装
          </Button>
        </Form>
      </Card>
    </div>
  );
}
