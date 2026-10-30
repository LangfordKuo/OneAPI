import { useQuery } from '@tanstack/react-query';
import { Loader2, Plus, Send, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { api, requestRaw } from '@/api/client';
import { readSse } from '@/api/sse';
import type { ModelsOutput, ProtocolName } from '@/api/types';
import { CodeBlock } from '@/components/common/code-block';
import { EmptyState } from '@/components/common/empty-state';
import { Field } from '@/components/common/field';
import { PageHeader } from '@/components/common/page-header';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import { Input } from '@/components/ui/input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { Switch } from '@/components/ui/switch';
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from '@/components/ui/tabs';
import { Textarea } from '@/components/ui/textarea';
import { formatLatency } from '@/lib/format';
import { cn } from '@/lib/utils';
import { errorMessage } from '@/lib/error';

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

const PROTOCOL_OPTIONS: Array<{ label: string; value: ProtocolName }> = [
  { label: 'Chat', value: 'chat' },
  { label: 'Responses', value: 'responses' },
  { label: 'Anthropic', value: 'anthropic' },
];

const ROLE_OPTIONS = [
  { value: 'user', label: 'User' },
  { value: 'assistant', label: 'Assistant' },
];

function mergeUsage(
  acc: PlaygroundResult,
  usage: Record<string, unknown> | null | undefined,
) {
  if (!usage) {
    return;
  }
  const input = (usage.input_tokens ?? usage.prompt_tokens) as
    | number
    | undefined;
  const output = (usage.output_tokens ?? usage.completion_tokens) as
    | number
    | undefined;
  const total = (usage.total_tokens ??
    (input != null && output != null
      ? input + output
      : undefined)) as number | undefined;

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
    const choices = payload.choices as
      | Array<Record<string, unknown>>
      | undefined;
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
      acc.error =
        typeof error.message === 'string' ? error.message : '上游返回错误';
    }
    return;
  }

  if (protocol === 'anthropic') {
    const type = event ?? (payload.type as string | undefined);
    if (type === 'message_start') {
      const message = payload.message as Record<string, unknown> | undefined;
      acc.model = typeof message?.model === 'string' ? message.model : acc.model;
      acc.responseId =
        typeof message?.id === 'string' ? message.id : acc.responseId;
      mergeUsage(acc, message?.usage as Record<string, unknown> | undefined);
    } else if (type === 'content_block_start') {
      const block = payload.content_block as
        | Record<string, unknown>
        | undefined;
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
      } else if (
        delta?.type === 'thinking_delta' &&
        typeof delta.thinking === 'string'
      ) {
        acc.reasoning += delta.thinking;
      } else if (
        delta?.type === 'input_json_delta' &&
        typeof delta.partial_json === 'string'
      ) {
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
      acc.error =
        typeof error?.message === 'string' ? error.message : '上游返回错误';
    }
    return;
  }

  const type = event ?? (payload.type as string | undefined);
  if (type === 'response.created') {
    const response = payload.response as Record<string, unknown> | undefined;
    acc.model = typeof response?.model === 'string' ? response.model : acc.model;
    acc.responseId =
      typeof response?.id === 'string' ? response.id : acc.responseId;
  } else if (
    type === 'response.output_text.delta' &&
    typeof payload.delta === 'string'
  ) {
    acc.text += payload.delta;
  } else if (
    type === 'response.reasoning_summary_text.delta' &&
    typeof payload.delta === 'string'
  ) {
    acc.reasoning += payload.delta;
  } else if (type === 'response.output_item.added') {
    const item = payload.item as Record<string, unknown> | undefined;
    if (item?.type === 'function_call') {
      const tool = ensureTool(
        acc,
        String(item.id ?? payload.output_index ?? ''),
      );
      tool.id = typeof item.call_id === 'string' ? item.call_id : '';
      tool.name = typeof item.name === 'string' ? item.name : '';
    }
  } else if (
    type === 'response.function_call_arguments.delta' &&
    typeof payload.delta === 'string'
  ) {
    const key = String(payload.item_id ?? `item-${payload.output_index ?? 0}`);
    ensureTool(acc, key).arguments += payload.delta;
  } else if (
    type === 'response.completed' ||
    type === 'response.incomplete'
  ) {
    const response = payload.response as Record<string, unknown> | undefined;
    acc.stopReason =
      typeof response?.status === 'string' ? response.status : acc.stopReason;
    mergeUsage(acc, response?.usage as Record<string, unknown> | undefined);
  } else if (type === 'response.failed') {
    const response = payload.response as Record<string, unknown> | undefined;
    const error = response?.error as Record<string, unknown> | undefined;
    acc.error =
      typeof error?.message === 'string' ? error.message : '上游返回错误';
  } else if (type === 'error') {
    acc.error =
      typeof payload.message === 'string' ? payload.message : '上游返回错误';
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
    const choices = payload.choices as
      | Array<Record<string, unknown>>
      | undefined;
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
    const content = payload.content as
      | Array<Record<string, unknown>>
      | undefined;
    for (const block of content ?? []) {
      if (block.type === 'text' && typeof block.text === 'string') {
        acc.text += block.text;
      } else if (
        block.type === 'thinking' &&
        typeof block.thinking === 'string'
      ) {
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
  const output = payload.output as
    | Array<Record<string, unknown>>
    | undefined;
  for (const item of output ?? []) {
    if (item.type === 'message') {
      const content = item.content as
        | Array<Record<string, unknown>>
        | undefined;
      for (const part of content ?? []) {
        if (part.type === 'output_text' && typeof part.text === 'string') {
          acc.text += part.text;
        }
      }
    } else if (item.type === 'reasoning') {
      const summary = item.summary as
        | Array<Record<string, unknown>>
        | undefined;
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

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0 space-y-0.5">
      <p className="text-xs text-muted-foreground">{label}</p>
      <p className="truncate text-sm" title={value}>
        {value}
      </p>
    </div>
  );
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
  const abortRef = useRef<AbortController | null>(null);

  // Stop the upstream stream when the page unmounts.
  useEffect(() => {
    return () => abortRef.current?.abort();
  }, []);

  const modelsQuery = useQuery({
    queryKey: ['models'],
    queryFn: () => api.get<ModelsOutput>('/api/models'),
  });
  const effectiveModel = model ?? modelsQuery.data?.models[0];

  function updateMessage(id: number, patch: Partial<MessageDraft>) {
    setMessages((previous) =>
      previous.map((message) =>
        message.id === id ? { ...message, ...patch } : message,
      ),
    );
  }

  function buildRequest(): Record<string, unknown> {
    const filled = messages
      .map((message) => ({ ...message, content: message.content.trim() }))
      .filter((message) => message.content.length > 0);

    if (protocol === 'chat') {
      const chatMessages: Record<string, unknown>[] = [];
      if (system.trim()) {
        chatMessages.push({ role: 'system', content: system });
      }
      chatMessages.push(
        ...filled.map((message) => ({
          role: message.role,
          content: message.content,
        })),
      );
      return {
        protocol: 'chat',
        model: effectiveModel,
        stream: streaming,
        messages: chatMessages,
      };
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
              type:
                message.role === 'user' ? 'input_text' : 'output_text',
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
      messages: filled.map((message) => ({
        role: message.role,
        content: message.content,
      })),
    };
  }

  async function run() {
    if (!effectiveModel) {
      setResult({ ...EMPTY_RESULT, error: '请选择模型' });
      return;
    }

    setRunning(true);
    setResult(EMPTY_RESULT);
    setElapsedMs(null);
    setFirstTokenMs(null);

    // Cancel the previous run so two streams never write into the same result.
    abortRef.current?.abort();
    const controller = new AbortController();
    abortRef.current = controller;

    const started = performance.now();
    const acc: PlaygroundResult = { ...EMPTY_RESULT, tools: [] };
    let firstToken = false;
    const publish = () => setResult({ ...acc, tools: [...acc.tools] });

    try {
      // requestRaw keeps credentials, error extraction and 401 handling in one
      // place while leaving the body unread for the SSE loop below.
      const response = await requestRaw('/api/playground', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(buildRequest()),
        signal: controller.signal,
      });

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
          if (
            !firstToken &&
            (acc.text || acc.reasoning || acc.tools.length > 0)
          ) {
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
      // Aborts are expected when a newer run starts or the page unmounts.
      if (!controller.signal.aborted) {
        acc.error = errorMessage(error);
        publish();
      }
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
        setElapsedMs(Math.round(performance.now() - started));
        setRunning(false);
      }
    }
  }

  return (
    <>
      <PageHeader
        title="Playground"
        description="用同一份配置直接测试三种入站协议，查看协议转换与上游路由结果。"
        actions={
          <Badge variant="outline" className="font-normal text-muted-foreground">
            请求会计入日志
          </Badge>
        }
      />

      <div className="grid gap-4 xl:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        <div className="space-y-4">
          <Card>
            <CardHeader>
              <CardTitle>请求配置</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid grid-cols-3 gap-1 rounded-lg bg-muted p-1">
                {PROTOCOL_OPTIONS.map((option) => (
                  <button
                    key={option.value}
                    type="button"
                    onClick={() => setProtocol(option.value)}
                    className={cn(
                      'rounded-md px-2 py-1.5 text-sm font-medium transition-colors',
                      protocol === option.value
                        ? 'bg-background text-foreground shadow-sm'
                        : 'text-muted-foreground hover:text-foreground',
                    )}
                  >
                    {option.label}
                  </button>
                ))}
              </div>

              <Field label="模型" htmlFor="playground-model">
                <Input
                  id="playground-model"
                  list="playground-models"
                  value={effectiveModel ?? ''}
                  onChange={(event) => setModel(event.target.value)}
                  placeholder="选择或输入模型"
                  className="font-mono"
                />
                <datalist id="playground-models">
                  {(modelsQuery.data?.models ?? []).map((item) => (
                    <option key={item} value={item} />
                  ))}
                </datalist>
              </Field>

              <div className="flex flex-wrap items-center gap-4">
                <div className="flex items-center gap-2">
                  <Switch
                    checked={streaming}
                    onCheckedChange={setStreaming}
                    id="playground-stream"
                  />
                  <label
                    htmlFor="playground-stream"
                    className="text-sm font-medium"
                  >
                    流式
                  </label>
                </div>
                {protocol === 'anthropic' ? (
                  <div className="flex items-center gap-2">
                    <label
                      htmlFor="playground-maxtokens"
                      className="text-sm font-medium"
                    >
                      max_tokens
                    </label>
                    <Input
                      id="playground-maxtokens"
                      type="number"
                      min={1}
                      max={200_000}
                      value={maxTokens}
                      onChange={(event) =>
                        setMaxTokens(Number(event.target.value) || 4096)
                      }
                      className="w-28"
                    />
                  </div>
                ) : null}
              </div>

              <Field
                label="System / instructions（可选）"
                htmlFor="playground-system"
              >
                <Textarea
                  id="playground-system"
                  value={system}
                  onChange={(event) => setSystem(event.target.value)}
                  placeholder="你是一个乐于助人的助手。"
                  rows={3}
                  className="max-h-40"
                />
              </Field>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>消息</CardTitle>
            </CardHeader>
            <CardContent className="space-y-3">
              {messages.map((message) => (
                <div
                  key={message.id}
                  className="flex flex-col gap-2 rounded-lg border p-2 sm:flex-row sm:items-start"
                >
                  <Select
                    items={ROLE_OPTIONS}
                    value={message.role}
                    onValueChange={(value) =>
                      updateMessage(message.id, {
                        role: value as MessageDraft['role'],
                      })
                    }
                  >
                    <SelectTrigger size="sm" className="w-full sm:w-[118px]">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="user">User</SelectItem>
                      <SelectItem value="assistant">Assistant</SelectItem>
                    </SelectContent>
                  </Select>
                  <Textarea
                    value={message.content}
                    onChange={(event) =>
                      updateMessage(message.id, {
                        content: event.target.value,
                      })
                    }
                    placeholder="输入消息内容"
                    rows={3}
                    className="min-h-20 flex-1"
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="text-destructive hover:text-destructive"
                    disabled={messages.length <= 1}
                    onClick={() =>
                      setMessages((previous) =>
                        previous.filter((item) => item.id !== message.id),
                      )
                    }
                    title="删除消息"
                  >
                    <Trash2 />
                  </Button>
                </div>
              ))}

              <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setMessages((previous) => [
                      ...previous,
                      {
                        id: Date.now(),
                        role: 'assistant',
                        content: '',
                      },
                    ])
                  }
                >
                  <Plus />
                  添加消息
                </Button>
                <Button
                  type="button"
                  onClick={() => void run()}
                  disabled={running}
                >
                  {running ? (
                    <Loader2 className="animate-spin" />
                  ) : (
                    <Send />
                  )}
                  发送
                </Button>
              </div>
            </CardContent>
          </Card>
        </div>

        <Card className="self-start">
          <CardHeader>
            <CardTitle>响应</CardTitle>
          </CardHeader>
          <CardContent className="space-y-4">
            {result.error ? (
              <Alert variant="destructive">
                <AlertTitle>请求失败</AlertTitle>
                <AlertDescription className="break-words">
                  {result.error}
                </AlertDescription>
              </Alert>
            ) : null}

            <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-3">
              <Stat label="渠道" value={result.channel ?? '-'} />
              <Stat label="上游模型" value={result.upstreamModel ?? '-'} />
              <Stat
                label="模型"
                value={result.model ?? effectiveModel ?? '-'}
              />
              <Stat label="结束原因" value={result.stopReason ?? '-'} />
              <Stat
                label="耗时"
                value={elapsedMs == null ? '-' : formatLatency(elapsedMs)}
              />
              <Stat
                label="首 token"
                value={
                  firstTokenMs == null ? '-' : formatLatency(firstTokenMs)
                }
              />
              {result.responseId ? (
                <div className="col-span-2 min-w-0 space-y-0.5 sm:col-span-3">
                  <dt className="text-xs text-muted-foreground">响应 ID</dt>
                  <dd className="truncate font-mono text-xs">
                    {result.responseId}
                  </dd>
                </div>
              ) : null}
            </dl>

            <Tabs defaultValue="text">
              <TabsList className="w-full">
                <TabsTrigger value="text">文本</TabsTrigger>
                <TabsTrigger value="reasoning">推理</TabsTrigger>
                <TabsTrigger value="tools">
                  工具调用 ({result.tools.length})
                </TabsTrigger>
                <TabsTrigger value="usage">用量</TabsTrigger>
              </TabsList>

              <TabsContent value="text" className="pt-3">
                {result.text ? (
                  <CodeBlock>{result.text}</CodeBlock>
                ) : (
                  <EmptyState title="暂无文本" />
                )}
              </TabsContent>

              <TabsContent value="reasoning" className="pt-3">
                {result.reasoning ? (
                  <CodeBlock>{result.reasoning}</CodeBlock>
                ) : (
                  <EmptyState title="暂无推理内容" />
                )}
              </TabsContent>

              <TabsContent value="tools" className="space-y-2 pt-3">
                {result.tools.length > 0 ? (
                  result.tools.map((tool) => (
                    <div
                      key={tool.key}
                      className="space-y-2 rounded-lg border p-3"
                    >
                      <div className="flex flex-wrap items-center gap-2">
                        <Badge variant="outline">
                          {tool.name || 'function'}
                        </Badge>
                        <span className="font-mono text-xs text-muted-foreground">
                          {tool.id || '-'}
                        </span>
                      </div>
                      <CodeBlock showCopy={false}>
                        {prettyArguments(tool.arguments) || '（无参数）'}
                      </CodeBlock>
                    </div>
                  ))
                ) : (
                  <EmptyState title="没有工具调用" />
                )}
              </TabsContent>

              <TabsContent value="usage" className="pt-3">
                {result.usage ? (
                  <dl className="grid grid-cols-3 gap-3 rounded-lg border p-3">
                    <Stat
                      label="输入"
                      value={String(result.usage.input ?? '-')}
                    />
                    <Stat
                      label="输出"
                      value={String(result.usage.output ?? '-')}
                    />
                    <Stat
                      label="总计"
                      value={String(result.usage.total ?? '-')}
                    />
                  </dl>
                ) : (
                  <EmptyState title="暂无用量信息" />
                )}
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>
      </div>
    </>
  );
}
