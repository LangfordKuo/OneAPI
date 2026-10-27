import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Alert,
  App,
  Button,
  DatePicker,
  Form,
  Input,
  Modal,
  Popconfirm,
  Space,
  Switch,
  Table,
  Tag,
  Typography,
} from 'antd';
import type { TableColumnsType } from 'antd';
import dayjs, { type Dayjs } from 'dayjs';
import { useState } from 'react';
import { api } from '../api/client';
import type { ApiKey } from '../api/types';

interface KeyFormValues {
  name: string;
  expires_at?: Dayjs;
  enabled?: boolean;
}

interface CreatePayload {
  name: string;
  expires_at: number | null;
  enabled: boolean;
}

function formatTime(value: number | null) {
  return value ? dayjs(value).format('YYYY-MM-DD HH:mm') : '-';
}

export default function Keys() {
  const { message } = App.useApp();
  const queryClient = useQueryClient();
  const [createOpen, setCreateOpen] = useState(false);
  const [createdKey, setCreatedKey] = useState<string | null>(null);
  const [form] = Form.useForm<KeyFormValues>();

  const keysQuery = useQuery({
    queryKey: ['keys'],
    queryFn: () => api.get<ApiKey[]>('/api/keys'),
  });

  const createMutation = useMutation({
    mutationFn: (payload: CreatePayload) => api.post<ApiKey>('/api/keys', payload),
    onSuccess: (data) => {
      setCreateOpen(false);
      form.resetFields();
      if (data.key) {
        setCreatedKey(data.key);
      }
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const updateMutation = useMutation({
    mutationFn: ({ id, enabled }: { id: number; enabled: boolean }) =>
      api.put<ApiKey>(`/api/keys/${id}`, { enabled }),
    onSuccess: () => {
      message.success('已更新');
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const deleteMutation = useMutation({
    mutationFn: (id: number) => api.delete(`/api/keys/${id}`),
    onSuccess: () => {
      message.success('已删除');
      queryClient.invalidateQueries({ queryKey: ['keys'] });
    },
    onError: (error) =>
      message.error(error instanceof Error ? error.message : String(error)),
  });

  const columns: TableColumnsType<ApiKey> = [
    { title: '名称', dataIndex: 'name' },
    {
      title: 'Key 前缀',
      dataIndex: 'key_prefix',
      render: (value: string) => <Typography.Text code>{value}…</Typography.Text>,
    },
    {
      title: '状态',
      dataIndex: 'enabled',
      render: (enabled: boolean) => (
        <Tag color={enabled ? 'green' : 'default'}>{enabled ? '启用' : '停用'}</Tag>
      ),
    },
    {
      title: '过期时间',
      dataIndex: 'expires_at',
      render: (value: number | null) => formatTime(value),
    },
    {
      title: '最后使用',
      dataIndex: 'last_used_at',
      render: (value: number | null) => formatTime(value),
    },
    {
      title: '创建时间',
      dataIndex: 'created_at',
      render: (value: number) => formatTime(value),
    },
    {
      title: '操作',
      render: (_, record) => (
        <Space>
          <Switch
            size="small"
            checked={record.enabled}
            onChange={(checked) =>
              updateMutation.mutate({ id: record.id, enabled: checked })
            }
          />
          <Popconfirm
            title="删除该 Key？"
            description="删除后使用该 Key 的客户端会立即失败。"
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
        <Button
          type="primary"
          onClick={() => {
            form.resetFields();
            form.setFieldsValue({ enabled: true });
            setCreateOpen(true);
          }}
        >
          新建 API Key
        </Button>
        <Typography.Text type="secondary">完整 Key 只在创建时显示一次。</Typography.Text>
      </Space>
      <Table
        rowKey="id"
        columns={columns}
        dataSource={keysQuery.data ?? []}
        loading={keysQuery.isLoading}
        pagination={false}
      />

      <Modal
        title="新建 API Key"
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => form.submit()}
        confirmLoading={createMutation.isPending}
        destroyOnHidden
      >
        <Form
          form={form}
          layout="vertical"
          onFinish={(values: KeyFormValues) =>
            createMutation.mutate({
              name: values.name,
              expires_at: values.expires_at ? values.expires_at.valueOf() : null,
              enabled: values.enabled ?? true,
            })
          }
        >
          <Form.Item
            name="name"
            label="名称"
            rules={[{ required: true, message: '请输入名称' }]}
          >
            <Input placeholder="例如：本地开发" />
          </Form.Item>
          <Form.Item name="expires_at" label="过期时间（可选）">
            <DatePicker showTime style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="enabled" label="启用" valuePropName="checked">
            <Switch />
          </Form.Item>
        </Form>
      </Modal>

      <Modal
        title="API Key 已创建"
        open={createdKey !== null}
        onCancel={() => setCreatedKey(null)}
        footer={
          <Button type="primary" onClick={() => setCreatedKey(null)}>
            我已保存
          </Button>
        }
        destroyOnHidden
      >
        <Alert
          type="warning"
          showIcon
          message="完整 Key 只显示一次，请立即复制保存。"
          style={{ marginBottom: 12 }}
        />
        <Typography.Text
          code
          copyable={{ text: createdKey ?? '' }}
          style={{ wordBreak: 'break-all' }}
        >
          {createdKey}
        </Typography.Text>
      </Modal>
    </>
  );
}
