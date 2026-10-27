import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Alert,
  App,
  AutoComplete,
  Button,
  Card,
  Descriptions,
  Form,
  Input,
  InputNumber,
  Select,
  Space,
  Switch,
  Tag,
  Typography,
} from 'antd';
import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type {
  Settings,
  UpdateApplyResult,
  UpdateInfo,
} from '../api/types';

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
  const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
  const [updateMessage, setUpdateMessage] = useState<string | null>(null);

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

  const checkUpdateMutation = useMutation({
    mutationFn: () => api.get<UpdateInfo>('/api/update/check'),
    onSuccess: (data) => {
      setUpdateInfo(data);
      setUpdateMessage(null);
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const applyUpdateMutation = useMutation({
    mutationFn: () => api.post<UpdateApplyResult>('/api/update/apply'),
    onSuccess: (data) => {
      setUpdateMessage(data.message);
      window.setTimeout(() => window.location.assign('/'), 3000);
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
          onFinish={(values: Settings) =>
            saveMutation.mutate({
              ...values,
              client_ip_header: values.client_ip_header ?? '',
            })
          }
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
          <Form.Item
            name="update_source"
            label="更新源"
            extra="修改后请先保存设置，再检查更新"
          >
            <Select
              options={[
                { value: 'github', label: 'GitHub 官方' },
                { value: 'ghproxy', label: '国内加速（gh-proxy.com）' },
              ]}
            />
          </Form.Item>
          <Form.Item
            name="client_ip_header"
            label="客户端 IP 请求头"
            extra="仅当所有流量都经过可信反向代理或 Cloudflare 时配置；留空则使用 TCP 对端地址。伪造该请求头可绕过登录限速。"
          >
            <AutoComplete
              allowClear
              placeholder="例如 CF-Connecting-IP"
              options={[
                {
                  value: 'cf-connecting-ip',
                  label: 'CF-Connecting-IP（Cloudflare）',
                },
                { value: 'x-forwarded-for', label: 'X-Forwarded-For' },
                { value: 'x-real-ip', label: 'X-Real-IP' },
                { value: 'true-client-ip', label: 'True-Client-IP' },
              ]}
            />
          </Form.Item>
          <Button type="primary" htmlType="submit" loading={saveMutation.isPending}>
            保存设置
          </Button>
        </Form>
      </Card>

      <Card title="软件更新">
        <Space direction="vertical" size={12} style={{ width: '100%' }}>
          <Space>
            <Button
              onClick={() => checkUpdateMutation.mutate()}
              loading={checkUpdateMutation.isPending}
            >
              检查更新
            </Button>
            {updateInfo ? (
              <Tag color={updateInfo.update_available ? 'orange' : 'green'}>
                {updateInfo.update_available ? '发现新版本' : '已是最新版本'}
              </Tag>
            ) : null}
          </Space>

          {updateInfo ? (
            <>
              <Descriptions column={2} size="small" bordered>
                <Descriptions.Item label="当前版本">
                  {updateInfo.current_version}
                </Descriptions.Item>
                <Descriptions.Item label="最新版本">
                  {updateInfo.latest_version}
                </Descriptions.Item>
                <Descriptions.Item label="更新源">
                  {updateInfo.source === 'ghproxy'
                    ? 'gh-proxy.com 加速'
                    : 'GitHub 官方'}
                </Descriptions.Item>
                <Descriptions.Item label="发布时间">
                  {updateInfo.published_at ?? '-'}
                </Descriptions.Item>
                <Descriptions.Item label="安装包" span={2}>
                  {updateInfo.asset_name ?? '当前平台暂不支持'}
                </Descriptions.Item>
              </Descriptions>

              {updateInfo.release_notes ? (
                <>
                  <Typography.Text strong>更新说明</Typography.Text>
                  <pre className="code-block">{updateInfo.release_notes}</pre>
                </>
              ) : null}

              {updateInfo.update_available ? (
                <Button
                  type="primary"
                  danger
                  onClick={() => applyUpdateMutation.mutate()}
                  loading={applyUpdateMutation.isPending}
                  disabled={!updateInfo.asset_name}
                >
                  一键更新并重启
                </Button>
              ) : null}
            </>
          ) : null}

          {updateMessage ? (
            <Alert
              type="success"
              showIcon
              message={updateMessage}
              description="服务会自动重启，页面将在几秒后刷新。"
            />
          ) : null}

          <Alert
            type="info"
            showIcon
            message="更新说明"
            description="更新会下载 SHA256SUMS 校验安装包，替换当前二进制并自动重启。容器部署请拉取新镜像；Windows 更新时需要保证程序目录可写。"
          />
        </Space>
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
