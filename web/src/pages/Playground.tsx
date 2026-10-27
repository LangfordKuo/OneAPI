import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import {
  Alert,
  AutoComplete,
  Button,
  Card,
  Col,
  Descriptions,
  Empty,
  Input,
  InputNumber,
  Row,
  Segmented,
  Select,
  Space,
  Switch,
  Tabs,
  Tag,
  Typography,
} from 'antd';
import { api } from '../api/client';
import { readSse } from '../api/sse';
import type { ModelsOutput, ProtocolName } from '../api/types';

interface MessageDraft {
  id: number;
  role: 'user' | 'assistant';
  content: string;
}

interface ToolState {
  key: string;
  id: string;
  name: string;
  arguments: string;
}

interface UsageState {
  input?: number;
  output?: number;
  total?: number;
}

interface PlaygroundResult {
  text: string;
  reasoning: string;
  tools: ToolState[];
  usage: UsageState | null;
  channel: string | null;
  upstreamModel: string | null;
  model: string | null;
  responseId: string | null;
  stopReason: string | null;
  error: string | null;
}

const EMPTY_RESULT: PlaygroundResult = {
  text: '',
  reasoning: '',
  tools: [],
  usage: null,
  channel: null,
  upstreamModel: null,
  model: null,
  responseId: null,
  stopReason: null,
  error: null,
};

const PROTOCOL_OPTIONS = [
  { label: 'Chat', value: 'chat' },
  { label: 'Responses', value: 'responses' },
  { label: 'Anthropic', value: 'anthropic' },
];

function mergeUsage(acc: PlaygroundResult, usage: Record<string, unknown> | null | undefined) {
  if (!usage) {
    return;
  }
  const input = (usage.input_tokens ?? usage.prompt_tokens) as number | undefined;
  const output = (usage.output_tokens ?? usage.completion_tokens) as number | undefined;
  const total = (usage.total_tokens ??
    (input != null && output != null ? input + output : undefined)) as number | undefined;

  if (input == null && output == null && total == null) {
    return;
  }
  acc.usage = {
    input: input ?? acc.usage?.input,
    output: output ?? acc.usage?.output,
    total: total ?? acc.usage?.total,
  };
}

function ensureTool(acc: PlaygroundResult, key: string): ToolState {
  let tool = acc.tools.find((item) => item.key === key);
  if (!tool) {
    tool = { key, id: '', name: '', arguments: '' };
    acc.tools.push(tool);
  }
  return tool;
}

function applyChatTool(
  acc: PlaygroundResult,
  index: number,
  id?: string,
  name?: string,
  argumentsText?: string,
) {
  const tool = ensureTool(acc, `tool-${index}`);
  if (id) {
    tool.id = id;
  }
  if (name) {
    tool.name = name;
  }
  if (argumentsText) {
    tool.arguments += argumentsText;
  }
}

function applyEvent(
  protocol: ProtocolName,
  event: string | undefined,
  payload: Record<string, unknown>,
  acc: PlaygroundResult,
) {
  if (protocol === 'chat') {
    const choices = payload.choices as Array<Record<string, unknown>> | undefined;
    const choice = choices?.[0];
    const delta = choice?.delta as Record<string, unknown> | undefined;
    if (typeof delta?.content === 'string') {
      acc.text += delta.content;
    }
    const reasoning = delta?.reasoning_content ?? delta?.reasoning;
    if (typeof reasoning === 'string') {
      acc.reasoning += reasoning;
    }
    if (Array.isArray(delta?.tool_calls)) {
      for (const call of delta.tool_calls as Array<Record<string, unknown>>) {
        const fn = call.function as Record<string, unknown> | undefined;
        applyChatTool(
          acc,
          Number(call.index ?? 0),
          typeof call.id === 'string' ? call.id : undefined,
          typeof fn?.name === 'string' ? fn.name : undefined,
          typeof fn?.arguments === 'string' ? fn.arguments : undefined,
        );
      }
    }
    mergeUsage(acc, payload.usage as Record<string, unknown> | undefined);
    if (typeof choice?.finish_reason === 'string') {
      acc.stopReason = choice.finish_reason;
    }
    const error = payload.error as Record<string, unknown> | undefined;
    if (error) {
      acc.error = typeof error.message === 'string' ? error.message : '上游返回错误';
    }
    return;
  }

  if (protocol === 'anthropic') {
    const type = event ?? (payload.type as string | undefined);
    if (type === 'message_start') {
      const message = payload.message as Record<string, unknown> | undefined;
      acc.model = typeof message?.model === 'string' ? message.model : acc.model;
      acc.responseId = typeof message?.id === 'string' ? message.id : acc.responseId;
      mergeUsage(acc, message?.usage as Record<string, unknown> | undefined);
    } else if (type === 'content_block_start') {
      const block = payload.content_block as Record<string, unknown> | undefined;
      const index = Number(payload.index ?? 0);
      if (block?.type === 'tool_use') {
        const tool = ensureTool(acc, `block-${index}`);
        tool.id = typeof block.id === 'string' ? block.id : '';
        tool.name = typeof block.name === 'string' ? block.name : '';
      }
    } else if (type === 'content_block_delta') {
      const delta = payload.delta as Record<string, unknown> | undefined;
      const index = Number(payload.index ?? 0);
      if (delta?.type === 'text_delta' && typeof delta.text === 'string') {
        acc.text += delta.text;
      } else if (delta?.type === 'thinking_delta' && typeof delta.thinking === 'string') {
        acc.reasoning += delta.thinking;
      } else if (delta?.type === 'input_json_delta' && typeof delta.partial_json === 'string') {
        ensureTool(acc, `block-${index}`).arguments += delta.partial_json;
      }
    } else if (type === 'message_delta') {
      const delta = payload.delta as Record<string, unknown> | undefined;
      if (typeof delta?.stop_reason === 'string') {
        acc.stopReason = delta.stop_reason;
      }
      mergeUsage(acc, payload.usage as Record<string, unknown> | undefined);
    } else if (type === 'error') {
      const error = payload.error as Record<string, unknown> | undefined;
      acc.error = typeof error?.message === 'string' ? error.message : '上游返回错误';
    }
    return;
  }

  const type = event ?? (payload.type as string | undefined);
  if (type === 'response.created') {
    const response = payload.response as Record<string, unknown> | undefined;
    acc.model = typeof response?.model === 'string' ? response.model : acc.model;
    acc.responseId = typeof response?.id === 'string' ? response.id : acc.responseId;
  } else if (type === 'response.output_text.delta' && typeof payload.delta === 'string') {
    acc.text += payload.delta;
  } else if (
    type === 'response.reasoning_summary_text.delta' &&
    typeof payload.delta === 'string'
  ) {
    acc.reasoning += payload.delta;
  } else if (type === 'response.output_item.added') {
    const item = payload.item as Record<string, unknown> | undefined;
    if (item?.type === 'function_call') {
      const tool = ensureTool(acc, String(item.id ?? payload.output_index ?? ''));
      tool.id = typeof item.call_id === 'string' ? item.call_id : '';
      tool.name = typeof item.name === 'string' ? item.name : '';
    }
  } else if (
    type === 'response.function_call_arguments.delta' &&
    typeof payload.delta === 'string'
  ) {
    const key = String(payload.item_id ?? `item-${payload.output_index ?? 0}`);
    ensureTool(acc, key).arguments += payload.delta;
  } else if (type === 'response.completed' || type === 'response.incomplete') {
    const response = payload.response as Record<string, unknown> | undefined;
    acc.stopReason = typeof response?.status === 'string' ? response.status : acc.stopReason;
    mergeUsage(acc, response?.usage as Record<string, unknown> | undefined);
  } else if (type === 'response.failed') {
    const response = payload.response as Record<string, unknown> | undefined;
    const error = response?.error as Record<string, unknown> | undefined;
    acc.error = typeof error?.message === 'string' ? error.message : '上游返回错误';
  } else if (type === 'error') {
    acc.error = typeof payload.message === 'string' ? payload.message : '上游返回错误';
  }
}

function applyComplete(
  protocol: ProtocolName,
  payload: Record<string, unknown>,
  acc: PlaygroundResult,
) {
  if (protocol === 'chat') {
    acc.model = typeof payload.model === 'string' ? payload.model : null;
    acc.responseId = typeof payload.id === 'string' ? payload.id : null;
    const choices = payload.choices as Array<Record<string, unknown>> | undefined;
    const choice = choices?.[0];
    const message = choice?.message as Record<string, unknown> | undefined;
    if (typeof message?.content === 'string') {
      acc.text = message.content;
    }
    const reasoning = message?.reasoning_content ?? message?.reasoning;
    if (typeof reasoning === 'string') {
      acc.reasoning = reasoning;
    }
    if (Array.isArray(message?.tool_calls)) {
      for (const call of message.tool_calls as Array<Record<string, unknown>>) {
        const fn = call.function as Record<string, unknown> | undefined;
        acc.tools.push({
          key: String(call.id ?? acc.tools.length),
          id: typeof call.id === 'string' ? call.id : '',
          name: typeof fn?.name === 'string' ? fn.name : '',
          arguments: typeof fn?.arguments === 'string' ? fn.arguments : '',
        });
      }
    }
    mergeUsage(acc, payload.usage as Record<string, unknown> | undefined);
    if (typeof choice?.finish_reason === 'string') {
      acc.stopReason = choice.finish_reason;
    }
    return;
  }

  if (protocol === 'anthropic') {
    acc.model = typeof payload.model === 'string' ? payload.model : null;
    acc.responseId = typeof payload.id === 'string' ? payload.id : null;
    const content = payload.content as Array<Record<string, unknown>> | undefined;
    for (const block of content ?? []) {
      if (block.type === 'text' && typeof block.text === 'string') {
        acc.text += block.text;
      } else if (block.type === 'thinking' && typeof block.thinking === 'string') {
        acc.reasoning += block.thinking;
      } else if (block.type === 'tool_use') {
        acc.tools.push({
          key: String(block.id ?? acc.tools.length),
          id: typeof block.id === 'string' ? block.id : '',
          name: typeof block.name === 'string' ? block.name : '',
          arguments: JSON.stringify(block.input ?? {}),
        });
      }
    }
    mergeUsage(acc, payload.usage as Record<string, unknown> | undefined);
    if (typeof payload.stop_reason === 'string') {
      acc.stopReason = payload.stop_reason;
    }
    return;
  }

  acc.model = typeof payload.model === 'string' ? payload.model : null;
  acc.responseId = typeof payload.id === 'string' ? payload.id : null;
  const output = payload.output as Array<Record<string, unknown>> | undefined;
  for (const item of output ?? []) {
    if (item.type === 'message') {
      const content = item.content as Array<Record<string, unknown>> | undefined;
      for (const part of content ?? []) {
        if (part.type === 'output_text' && typeof part.text === 'string') {
          acc.text += part.text;
        }
      }
    } else if (item.type === 'reasoning') {
      const summary = item.summary as Array<Record<string, unknown>> | undefined;
      for (const part of summary ?? []) {
        if (typeof part.text === 'string') {
          acc.reasoning += part.text;
        }
      }
    } else if (item.type === 'function_call') {
      acc.tools.push({
        key: String(item.id ?? acc.tools.length),
        id: typeof item.call_id === 'string' ? item.call_id : '',
        name: typeof item.name === 'string' ? item.name : '',
        arguments: typeof item.arguments === 'string' ? item.arguments : '',
      });
    }
  }
  mergeUsage(acc, payload.usage as Record<string, unknown> | undefined);
  if (typeof payload.status === 'string') {
    acc.stopReason = payload.status;
  }
}

function prettyArguments(value: string): string {
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

export default function Playground() {
  const [protocol, setProtocol] = useState<ProtocolName>('chat');
  const [model, setModel] = useState<string>();
  const [system, setSystem] = useState('');
  const [messages, setMessages] = useState<MessageDraft[]>([
    { id: 1, role: 'user', content: '' },
  ]);
  const [streaming, setStreaming] = useState(true);
  const [maxTokens, setMaxTokens] = useState(4096);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<PlaygroundResult>(EMPTY_RESULT);
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const [firstTokenMs, setFirstTokenMs] = useState<number | null>(null);

  const modelsQuery = useQuery({
    queryKey: ['models'],
    queryFn: () => api.get<ModelsOutput>('/api/models'),
  });
  const effectiveModel = model ?? modelsQuery.data?.models[0];

  const updateMessage = (id: number, patch: Partial<MessageDraft>) => {
    setMessages((previous) =>
      previous.map((message) => (message.id === id ? { ...message, ...patch } : message)),
    );
  };

  const buildRequest = (): Record<string, unknown> => {
    const filled = messages
      .map((message) => ({ ...message, content: message.content.trim() }))
      .filter((message) => message.content.length > 0);

    if (protocol === 'chat') {
      const chatMessages: Record<string, unknown>[] = [];
      if (system.trim()) {
        chatMessages.push({ role: 'system', content: system });
      }
      chatMessages.push(
        ...filled.map((message) => ({ role: message.role, content: message.content })),
      );
      return { protocol: 'chat', model: effectiveModel, stream: streaming, messages: chatMessages };
    }

    if (protocol === 'responses') {
      return {
        protocol: 'responses',
        model: effectiveModel,
        stream: streaming,
        instructions: system.trim() || undefined,
        input: filled.map((message) => ({
          type: 'message',
          role: message.role,
          content: [
            {
              type: message.role === 'user' ? 'input_text' : 'output_text',
              text: message.content,
            },
          ],
        })),
      };
    }

    return {
      protocol: 'anthropic',
      model: effectiveModel,
      stream: streaming,
      max_tokens: maxTokens,
      system: system.trim() || undefined,
      messages: filled.map((message) => ({ role: message.role, content: message.content })),
    };
  };

  const run = async () => {
    if (!effectiveModel) {
      setResult({ ...EMPTY_RESULT, error: '请选择模型' });
      return;
    }

    setRunning(true);
    setResult(EMPTY_RESULT);
    setElapsedMs(null);
    setFirstTokenMs(null);

    const started = performance.now();
    const acc: PlaygroundResult = { ...EMPTY_RESULT, tools: [] };
    let firstToken = false;
    const publish = () => setResult({ ...acc, tools: [...acc.tools] });

    try {
      const response = await fetch('/api/playground', {
        method: 'POST',
        credentials: 'include',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(buildRequest()),
      });

      if (!response.ok) {
        let message = `HTTP ${response.status}`;
        try {
          const body = (await response.json()) as { error?: { message?: string } };
          message = body.error?.message ?? message;
        } catch {
          // Keep the status text.
        }
        acc.error = message;
        publish();
        return;
      }

      acc.channel = response.headers.get('x-oneapi-channel');
      acc.upstreamModel = response.headers.get('x-oneapi-upstream-model');

      if (streaming) {
        if (!response.body) {
          throw new Error('响应没有流式内容');
        }
        for await (const event of readSse(response)) {
          if (event.data === '[DONE]') {
            break;
          }
          let payload: Record<string, unknown>;
          try {
            payload = JSON.parse(event.data) as Record<string, unknown>;
          } catch {
            continue;
          }
          applyEvent(protocol, event.event, payload, acc);
          if (!firstToken && (acc.text || acc.reasoning || acc.tools.length > 0)) {
            firstToken = true;
            setFirstTokenMs(Math.round(performance.now() - started));
          }
          publish();
        }
      } else {
        const payload = (await response.json()) as Record<string, unknown>;
        applyComplete(protocol, payload, acc);
        publish();
      }
    } catch (error) {
      acc.error = error instanceof Error ? error.message : String(error);
      publish();
    } finally {
      setElapsedMs(Math.round(performance.now() - started));
      setRunning(false);
    }
  };

  return (
    <Row gutter={[16, 16]}>
      <Col xs={24} xl={11}>
        <Card title="请求配置" size="small">
          <Space direction="vertical" style={{ width: '100%' }} size="middle">
            <Segmented
              block
              value={protocol}
              options={PROTOCOL_OPTIONS}
              onChange={(value) => setProtocol(value as ProtocolName)}
            />
            <AutoComplete
              placeholder="选择或输入模型"
              value={effectiveModel}
              options={(modelsQuery.data?.models ?? []).map((item) => ({
                value: item,
                label: item,
              }))}
              onChange={(value) => setModel(value)}
              style={{ width: '100%' }}
            />
            <Space>
              <Typography.Text>流式</Typography.Text>
              <Switch checked={streaming} onChange={setStreaming} />
              {protocol === 'anthropic' ? (
                <>
                  <Typography.Text>max_tokens</Typography.Text>
                  <InputNumber
                    min={1}
                    max={200_000}
                    value={maxTokens}
                    onChange={(value) => setMaxTokens(Number(value ?? 4096))}
                  />
                </>
              ) : null}
            </Space>
            <Input.TextArea
              placeholder="System / instructions（可选）"
              autoSize={{ minRows: 2, maxRows: 5 }}
              value={system}
              onChange={(event) => setSystem(event.target.value)}
            />
          </Space>
        </Card>

        <Card
          title="消息"
          size="small"
          style={{ marginTop: 16 }}
          extra={
            <Button
              size="small"
              onClick={() =>
                setMessages((previous) => [
                  ...previous,
                  { id: Date.now(), role: 'assistant', content: '' },
                ])
              }
            >
              添加消息
            </Button>
          }
        >
          <Space direction="vertical" style={{ width: '100%' }} size="middle">
            {messages.map((message) => (
              <Space.Compact key={message.id} style={{ width: '100%', alignItems: 'flex-start' }}>
                <Select
                  value={message.role}
                  style={{ width: 110 }}
                  options={[
                    { value: 'user', label: 'User' },
                    { value: 'assistant', label: 'Assistant' },
                  ]}
                  onChange={(value) => updateMessage(message.id, { role: value })}
                />
                <Input.TextArea
                  autoSize={{ minRows: 2, maxRows: 8 }}
                  value={message.content}
                  onChange={(event) =>
                    updateMessage(message.id, { content: event.target.value })
                  }
                />
                <Button
                  danger
                  disabled={messages.length <= 1}
                  onClick={() =>
                    setMessages((previous) =>
                      previous.filter((item) => item.id !== message.id),
                    )
                  }
                >
                  删除
                </Button>
              </Space.Compact>
            ))}
            <Button type="primary" block loading={running} onClick={() => void run()}>
              发送
            </Button>
          </Space>
        </Card>
      </Col>

      <Col xs={24} xl={13}>
        <Card title="响应" size="small">
          {result.error ? (
            <Alert type="error" showIcon message="请求失败" description={result.error} />
          ) : null}

          <Descriptions column={2} size="small" style={{ marginTop: result.error ? 16 : 0 }}>
            <Descriptions.Item label="渠道">{result.channel ?? '-'}</Descriptions.Item>
            <Descriptions.Item label="上游模型">
              {result.upstreamModel ?? '-'}
            </Descriptions.Item>
            <Descriptions.Item label="模型">{result.model ?? effectiveModel ?? '-'}</Descriptions.Item>
            <Descriptions.Item label="结束原因">{result.stopReason ?? '-'}</Descriptions.Item>
            <Descriptions.Item label="耗时">
              {elapsedMs == null ? '-' : `${elapsedMs} ms`}
            </Descriptions.Item>
            <Descriptions.Item label="首 token">
              {firstTokenMs == null ? '-' : `${firstTokenMs} ms`}
            </Descriptions.Item>
            {result.responseId ? (
              <Descriptions.Item label="响应 ID" span={2}>
                <Typography.Text copyable>{result.responseId}</Typography.Text>
              </Descriptions.Item>
            ) : null}
          </Descriptions>

          <Tabs
            style={{ marginTop: 8 }}
            items={[
              {
                key: 'text',
                label: '文本',
                children: result.text ? (
                  <pre className="code-block">{result.text}</pre>
                ) : (
                  <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无文本" />
                ),
              },
              {
                key: 'reasoning',
                label: '推理',
                children: result.reasoning ? (
                  <pre className="code-block">{result.reasoning}</pre>
                ) : (
                  <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无推理内容" />
                ),
              },
              {
                key: 'tools',
                label: `工具调用 (${result.tools.length})`,
                children:
                  result.tools.length > 0 ? (
                    <Space direction="vertical" style={{ width: '100%' }}>
                      {result.tools.map((tool) => (
                        <Card key={tool.key} size="small" type="inner">
                          <Space>
                            <Tag color="blue">{tool.name || 'function'}</Tag>
                            <Typography.Text type="secondary">{tool.id || '-'}</Typography.Text>
                          </Space>
                          <pre className="code-block">
                            {prettyArguments(tool.arguments) || '（无参数）'}
                          </pre>
                        </Card>
                      ))}
                    </Space>
                  ) : (
                    <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="没有工具调用" />
                  ),
              },
              {
                key: 'usage',
                label: '用量',
                children: result.usage ? (
                  <Descriptions column={1} size="small" bordered>
                    <Descriptions.Item label="输入">
                      {result.usage.input ?? '-'}
                    </Descriptions.Item>
                    <Descriptions.Item label="输出">
                      {result.usage.output ?? '-'}
                    </Descriptions.Item>
                    <Descriptions.Item label="总计">
                      {result.usage.total ?? '-'}
                    </Descriptions.Item>
                  </Descriptions>
                ) : (
                  <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description="暂无用量信息" />
                ),
              },
            ]}
          />
        </Card>
      </Col>
    </Row>
  );
}
