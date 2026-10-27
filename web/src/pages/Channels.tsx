import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  App,
  Button,
  Drawer,
  Form,
  Input,
  InputNumber,
  Modal,
  Popconfirm,
  Select,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
} from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { api } from '../api/client';
import type {
  Channel,
  ChannelKind,
  ChannelTestResult,
  ModelFetchResult,
} from '../api/types';
import KeyValueList from '../components/KeyValueList';

interface MappingRow {
  key: string;
  value: string;
}

interface ChannelFormValues {
  name: string;
  kind: ChannelKind;
  base_url: string;
  api_key?: string;
  auth_header?: 'bearer' | 'x-api-key';
  models?: string[];
  model_mapping?: MappingRow[];
  extra_headers?: MappingRow[];
  priority?: number;
  weight?: number;
  timeout_ms?: number;
  proxy_url?: string;
  enabled?: boolean;
}

const kindLabels: Record<ChannelKind, string> = {
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
  anthropic: 'Anthropic',
};

function toPayload(values: ChannelFormValues) {
  return {
    name: values.name,
    kind: values.kind,
    base_url: values.base_url,
    api_key: values.api_key?.trim() || undefined,
    auth_header: values.auth_header,
    models: values.models ?? [],
    model_mapping: Object.fromEntries(
      (values.model_mapping ?? [])
        .filter((row) => row?.key?.trim())
        .map((row) => [row.key.trim(), row.value.trim()]),
    ),
    extra_headers: Object.fromEntries(
      (values.extra_headers ?? [])
        .filter((row) => row?.key?.trim())
        .map((row) => [row.key.trim(), row.value.trim()]),
    ),
    priority: values.priority ?? 0,
    weight: values.weight ?? 1,
    timeout_ms: values.timeout_ms ?? null,
    proxy_url: values.proxy_url?.trim() || null,
    enabled: values.enabled ?? true,
  };
}

export default function Channels() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [form] = Form.useForm<ChannelFormValues>();
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Channel | null>(null);
  const [fetchedModels, setFetchedModels] = useState<string[]>([]);
  const [selectedModels, setSelectedModels] = useState<string[]>([]);
  const [modelModalOpen, setModelModalOpen] = useState(false);

  const channelsQuery = useQuery({
    queryKey: ['channels'],
    queryFn: () => api.get<Channel[]>('/api/channels'),
  });

  const saveMutation = useMutation({
    mutationFn: ({ id, values }: { id?: number; values: ChannelFormValues }) => {
      const payload = toPayload(values);
      return id
        ? api.put<Channel>(`/api/channels/${id}`, payload)
        : api.post<Channel>('/api/channels', payload);
    },
    onSuccess: () => {
      message.success('渠道已保存');
      setOpen(false);
      setEditing(null);
      form.resetFields();
      queryClient.invalidateQueries({ queryKey: ['channels'] });
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/api/channels/${id}`),
    onSuccess: () => {
      message.success('渠道已删除');
      queryClient.invalidateQueries({ queryKey: ['channels'] });
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const testMutation = useMutation({
    mutationFn: (id: number) =>
      api.post<ChannelTestResult>(`/api/channels/${id}/test`),
    onSuccess: (result) => {
      if (result.ok) {
        message.success(`连通成功，${result.latency_ms} ms`);
      } else {
        message.error(
          `测试失败（${result.status || '网络错误'}）：${result.message}`,
        );
      }
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const fetchModelsMutation = useMutation({
    mutationFn: (id: number) =>
      api.post<ModelFetchResult>(`/api/channels/${id}/fetch-models`),
    onSuccess: (result) => {
      if (result.models.length === 0) {
        message.warning(result.message || '未拉取到模型');
        return;
      }
      setFetchedModels(result.models);
      setSelectedModels(result.models);
      setModelModalOpen(true);
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  function openCreate() {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({
      kind: 'openai_chat',
      auth_header: 'bearer',
      models: [],
      priority: 0,
      weight: 1,
      enabled: true,
    });
    setOpen(true);
  }

  function openEdit(channel: Channel) {
    setEditing(channel);
    form.setFieldsValue({
      name: channel.name,
      kind: channel.kind,
      base_url: channel.base_url,
      api_key: undefined,
      auth_header: channel.auth_header,
      models: channel.models,
      model_mapping: Object.entries(channel.model_mapping).map(([key, value]) => ({
        key,
        value,
      })),
      extra_headers: Object.entries(channel.extra_headers).map(([key, value]) => ({
        key,
        value,
      })),
      priority: channel.priority,
      weight: channel.weight,
      timeout_ms: channel.timeout_ms ?? undefined,
      proxy_url: channel.proxy_url ?? undefined,
      enabled: channel.enabled,
    });
    setOpen(true);
  }

  const columns: TableColumnsType<Channel> = [
    {
      title: '名称',
      dataIndex: 'name',
      render: (name: string, record) => (
        <Space direction="vertical" size={0}>
          <Typography.Text strong>{name}</Typography.Text>
          <Typography.Text type="secondary">{record.api_key_masked}</Typography.Text>
        </Space>
      ),
    },
    {
      title: '类型',
      dataIndex: 'kind',
      render: (kind: ChannelKind) => <Tag>{kindLabels[kind]}</Tag>,
    },
    { title: 'Base URL', dataIndex: 'base_url', ellipsis: true },
    {
      title: '模型',
      dataIndex: 'models',
      render: (models: string[]) => `${models.length} 个`,
    },
    {
      title: '优先级 / 权重',
      render: (_, record) => `${record.priority} / ${record.weight}`,
    },
    {
      title: '状态',
      dataIndex: 'enabled',
      render: (enabled: boolean) => (
        <Tag color={enabled ? 'green' : 'default'}>{enabled ? '启用' : '停用'}</Tag>
      ),
    },
    {
      title: '操作',
      render: (_, record) => (
        <Space>
          <Button type="link" onClick={() => testMutation.mutate(record.id)}>
            测试
          </Button>
          <Button type="link" onClick={() => openEdit(record)}>
            编辑
          </Button>
          <Popconfirm
            title="删除该渠道？"
            description="删除后不可恢复。"
            onConfirm={() => deleteMutation.mutate(record.id)}
          >
            <Button type="link" danger>
              删除
            </Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <>
      <Space style={{ marginBottom: 16 }}>
        <Button type="primary" onClick={openCreate}>
          新建渠道
        </Button>
        <Typography.Text type="secondary">
          同一个模型可以配置多个渠道，请求会按优先级和权重路由。
        </Typography.Text>
      </Space>
      <Table
        rowKey="id"
        columns={columns}
        dataSource={channelsQuery.data ?? []}
        loading={channelsQuery.isLoading}
        pagination={false}
      />
      <Drawer
        title={editing ? '编辑渠道' : '新建渠道'}
        width={600}
        open={open}
        onClose={() => setOpen(false)}
        destroyOnHidden
      >
        {editing && (
          <Space style={{ marginBottom: 16 }}>
            <Button
              onClick={() => testMutation.mutate(editing.id)}
              loading={testMutation.isPending}
            >
              测试连通性
            </Button>
            <Button
              onClick={() => fetchModelsMutation.mutate(editing.id)}
              loading={fetchModelsMutation.isPending}
            >
              拉取模型
            </Button>
            <Typography.Text type="secondary">
              使用已保存的配置
            </Typography.Text>
          </Space>
        )}
        <Form
          form={form}
          layout="vertical"
          onFinish={(values: ChannelFormValues) =>
            saveMutation.mutate({ id: editing?.id, values })
          }
        >
          <Form.Item
            name="name"
            label="名称"
            rules={[{ required: true, message: '请输入渠道名称' }]}
          >
            <Input />
          </Form.Item>
          <Form.Item name="kind" label="协议类型" rules={[{ required: true }]}>
            <Select
              options={[
                { value: 'openai_chat', label: 'OpenAI Chat Completions' },
                { value: 'openai_responses', label: 'OpenAI Responses' },
                { value: 'anthropic', label: 'Anthropic Messages' },
              ]}
            />
          </Form.Item>
          <Form.Item
            name="base_url"
            label="Base URL"
            rules={[{ required: true, message: '请输入 Base URL' }]}
          >
            <Input placeholder="https://api.openai.com/v1" />
          </Form.Item>
          <Form.Item
            name="api_key"
            label="上游 API Key"
            extra={editing ? '留空表示保持原值' : undefined}
            rules={
              editing ? [] : [{ required: true, message: '请输入上游 API Key' }]
            }
          >
            <Input.Password
              placeholder={editing ? '留空保持不变' : 'sk-...'}
              autoComplete="new-password"
            />
          </Form.Item>
          <Form.Item name="auth_header" label="鉴权头">
            <Select
              options={[
                { value: 'bearer', label: 'Authorization: Bearer' },
                { value: 'x-api-key', label: 'x-api-key' },
              ]}
            />
          </Form.Item>
          <Form.Item
            name="models"
            label="模型"
            rules={[{ required: true, message: '至少填写一个模型' }]}
          >
            <Select
              mode="tags"
              tokenSeparators={[',']}
              placeholder="输入模型名后回车；* 表示接受任意模型"
            />
          </Form.Item>
          <Form.Item label="模型映射">
            <KeyValueList
              name="model_mapping"
              keyPlaceholder="客户端模型"
              valuePlaceholder="上游模型"
            />
          </Form.Item>
          <Form.Item label="自定义请求头">
            <KeyValueList
              name="extra_headers"
              keyPlaceholder="Header 名称"
              valuePlaceholder="Header 值"
            />
          </Form.Item>
          <Space size="large" style={{ display: 'flex' }}>
            <Form.Item name="priority" label="优先级">
              <InputNumber min={-1000} max={1000} />
            </Form.Item>
            <Form.Item name="weight" label="权重">
              <InputNumber min={1} max={1000} />
            </Form.Item>
            <Form.Item name="timeout_ms" label="超时 (ms)">
              <InputNumber min={1} />
            </Form.Item>
          </Space>
          <Form.Item name="proxy_url" label="代理 URL">
            <Input placeholder="http://127.0.0.1:7890 或 socks5://..." />
          </Form.Item>
          <Form.Item name="enabled" label="启用" valuePropName="checked">
            <Switch />
          </Form.Item>
          <Button type="primary" htmlType="submit" loading={saveMutation.isPending}>
            保存
          </Button>
        </Form>
      </Drawer>
      <Modal
        title="从上游拉取的模型"
        open={modelModalOpen}
        onCancel={() => setModelModalOpen(false)}
        onOk={() => {
          form.setFieldsValue({ models: selectedModels });
          setModelModalOpen(false);
          message.success('已填入表单，保存后生效');
        }}
        okText="填入表单"
        destroyOnHidden
      >
        <Select
          mode="multiple"
          value={selectedModels}
          onChange={setSelectedModels}
          options={fetchedModels.map((model) => ({
            value: model,
            label: model,
          }))}
          style={{ width: '100%' }}
          placeholder="选择要加入的模型"
        />
      </Modal>
    </>
  );
}
