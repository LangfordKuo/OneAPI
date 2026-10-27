import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  App,
  Button,
  Card,
  Form,
  Input,
  InputNumber,
  Space,
  Switch,
} from 'antd';
import { useEffect } from 'react';
import { api } from '../api/client';
import type { Settings } from '../api/types';

interface PasswordValues {
  current_password: string;
  new_password: string;
  confirm: string;
}

export default function SettingsPage() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<Settings>();
  const [passwordForm] = Form.useForm<PasswordValues>();

  const settingsQuery = useQuery({
    queryKey: ['settings'],
    queryFn: () => api.get<Settings>('/api/settings'),
  });

  useEffect(() => {
    if (settingsQuery.data) {
      form.setFieldsValue(settingsQuery.data);
    }
  }, [settingsQuery.data, form]);

  const saveMutation = useMutation({
    mutationFn: (values: Settings) => api.put<Settings>('/api/settings', values),
    onSuccess: (data) => {
      message.success('设置已保存');
      form.setFieldsValue(data);
      queryClient.setQueryData(['settings'], data);
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const passwordMutation = useMutation({
    mutationFn: (values: PasswordValues) =>
      api.put('/api/auth/password', {
        current_password: values.current_password,
        new_password: values.new_password,
      }),
    onSuccess: () => {
      message.success('密码已更新，其他会话已下线');
      passwordForm.resetFields();
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  return (
    <Space direction="vertical" size={16} style={{ display: 'flex' }}>
      <Card title="站点设置" loading={settingsQuery.isLoading}>
        <Form
          form={form}
          layout="vertical"
          onFinish={(values: Settings) => saveMutation.mutate(values)}
          style={{ maxWidth: 560 }}
        >
          <Form.Item
            name="site_name"
            label="站点名称"
            rules={[{ required: true, message: '请输入站点名称' }]}
          >
            <Input />
          </Form.Item>
          <Form.Item
            name="default_max_tokens"
            label="默认 max_tokens"
            extra="请求未指定 max_tokens 时使用"
          >
            <InputNumber min={1} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="nonstream_timeout_seconds" label="非流式超时（秒）">
            <InputNumber min={1} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="stream_idle_timeout_seconds" label="流式空闲超时（秒）">
            <InputNumber min={1} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item
            name="usage_log_retention_days"
            label="用量日志保留天数"
            extra="0 表示永久保留"
          >
            <InputNumber min={0} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item
            name="response_retention_days"
            label="Responses 保留天数"
            extra="0 表示永久保留"
          >
            <InputNumber min={0} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item
            name="store_request_bodies"
            label="记录请求/响应体"
            valuePropName="checked"
            extra="默认关闭；开启后保存请求/响应正文，单条最多 64KB，可在日志详情查看"
          >
            <Switch />
          </Form.Item>
          <Button type="primary" htmlType="submit" loading={saveMutation.isPending}>
            保存设置
          </Button>
        </Form>
      </Card>

      <Card title="修改密码">
        <Form
          form={passwordForm}
          layout="vertical"
          onFinish={(values: PasswordValues) => passwordMutation.mutate(values)}
          style={{ maxWidth: 420 }}
        >
          <Form.Item
            name="current_password"
            label="当前密码"
            rules={[{ required: true, message: '请输入当前密码' }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item
            name="new_password"
            label="新密码"
            rules={[
              { required: true, message: '请输入新密码' },
              { min: 8, message: '至少 8 个字符' },
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm"
            label="确认新密码"
            dependencies={['new_password']}
            rules={[
              { required: true, message: '请再次输入新密码' },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error('两次输入不一致'));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Button
            type="primary"
            htmlType="submit"
            loading={passwordMutation.isPending}
          >
            更新密码
          </Button>
        </Form>
      </Card>
    </Space>
  );
}
