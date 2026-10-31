import type {
  PlaygroundTool,
  PlaygroundUsage,
  ProtocolName,
} from '@/api/types';

/** Accumulated state for one streamed assistant turn. */
export interface TurnAccumulator {
  text: string;
  reasoning: string;
  tools: PlaygroundTool[];
  usage: PlaygroundUsage | null;
  channel: string | null;
  upstreamModel: string | null;
  model: string | null;
  responseId: string | null;
  stopReason: string | null;
  error: string | null;
}

export function emptyAccumulator(): TurnAccumulator {
  return {
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
}

function mergeUsage(
  acc: TurnAccumulator,
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
    (input != null && output != null ? input + output : undefined)) as
    | number
    | undefined;

  if (input == null && output == null && total == null) {
    return;
  }
  acc.usage = {
    input: input ?? acc.usage?.input,
    output: output ?? acc.usage?.output,
    total: total ?? acc.usage?.total,
  };
}

function ensureTool(acc: TurnAccumulator, key: string): PlaygroundTool {
  let tool = acc.tools.find((item) => item.key === key);
  if (!tool) {
    tool = { key, id: '', name: '', arguments: '' };
    acc.tools.push(tool);
  }
  return tool;
}

function applyChatTool(
  acc: TurnAccumulator,
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

export function applyEvent(
  protocol: ProtocolName,
  event: string | undefined,
  payload: Record<string, unknown>,
  acc: TurnAccumulator,
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

export function applyComplete(
  protocol: ProtocolName,
  payload: Record<string, unknown>,
  acc: TurnAccumulator,
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

export interface RequestConfig {
  protocol: ProtocolName;
  model: string;
  system: string;
  stream: boolean;
  maxTokens: number;
}

/**
 * Build the Playground proxy body from the visible conversation.
 *
 * Only final assistant content is replayed. Cross-protocol reasoning needs
 * protocol-specific signatures/items, so the stored reasoning stays in the
 * sidebar transcript instead of being sent upstream as a message.
 */
export function buildPlaygroundRequest(
  config: RequestConfig,
  messages: Array<{ role: 'user' | 'assistant'; content: string }>,
): Record<string, unknown> {
  const filled = messages
    .map((message) => ({ ...message, content: message.content.trim() }))
    .filter((message) => message.content.length > 0);

  if (config.protocol === 'chat') {
    const chatMessages: Record<string, unknown>[] = [];
    if (config.system.trim()) {
      chatMessages.push({ role: 'system', content: config.system });
    }
    chatMessages.push(
      ...filled.map((message) => ({
        role: message.role,
        content: message.content,
      })),
    );
    return {
      protocol: 'chat',
      model: config.model,
      stream: config.stream,
      messages: chatMessages,
    };
  }

  if (config.protocol === 'responses') {
    return {
      protocol: 'responses',
      model: config.model,
      stream: config.stream,
      instructions: config.system.trim() || undefined,
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
    model: config.model,
    stream: config.stream,
    max_tokens: config.maxTokens,
    system: config.system.trim() || undefined,
    messages: filled.map((message) => ({
      role: message.role,
      content: message.content,
    })),
  };
}

export function prettyArguments(value: string): string {
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}

/**
 * HTTP header values are byte strings. The gateway writes the channel name as
 * UTF-8, while `Headers.get` exposes each byte as a Latin-1 code unit, so a
 * Chinese channel name arrives mojibaked. Rebuild the bytes and decode them as
 * UTF-8; plain ASCII passes through unchanged.
 */
export function decodeRouteHeader(value: string | null): string | null {
  if (!value) {
    return null;
  }
  const chars = Array.from(value);
  if (chars.some((char) => char.charCodeAt(0) > 0xff)) {
    return value;
  }
  try {
    const bytes = Uint8Array.from(chars, (char) => char.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return value;
  }
}

