import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Brain,
  Check,
  ChevronDown,
  Copy,
  Loader2,
  Menu,
  Plus,
  Send,
  Settings2,
  Sparkles,
  Square,
  Trash2,
} from 'lucide-react';
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
} from 'react';
import { toast } from 'sonner';
import { api, requestRaw } from '@/api/client';
import { readSse } from '@/api/sse';
import type {
  ModelsOutput,
  PlaygroundMessage,
  PlaygroundMessageMeta,
  PlaygroundSession,
  PlaygroundSessionOutput,
  PlaygroundSessionsOutput,
  PlaygroundSessionSummary,
  ProtocolName,
} from '@/api/types';
import { CodeBlock } from '@/components/common/code-block';
import { ConfirmDialog } from '@/components/common/confirm-dialog';
import { Field } from '@/components/common/field';
import { Markdown } from '@/components/common/markdown';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Input } from '@/components/ui/input';
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
} from '@/components/ui/sheet';
import { Skeleton } from '@/components/ui/skeleton';
import { Switch } from '@/components/ui/switch';
import { Textarea } from '@/components/ui/textarea';
import { useNow } from '@/hooks/useNow';
import { errorMessage } from '@/lib/error';
import { formatSeconds, formatTokens } from '@/lib/format';
import {
  applyComplete,
  applyEvent,
  buildPlaygroundRequest,
  decodeRouteHeader,
  emptyAccumulator,
  prettyArguments,
  type TurnAccumulator,
} from '@/lib/playground-stream';
import { cn } from '@/lib/utils';

interface ChatMessage extends PlaygroundMessage {
  uid: string;
}

interface ChatSession extends Omit<PlaygroundSession, 'messages'> {
  messages: ChatMessage[];
}

interface Draft {
  protocol: ProtocolName;
  model?: string;
  system: string;
  stream: boolean;
  maxTokens: number;
}

interface RunState {
  token: string;
  sessionId: string;
  uid: string;
}

const PREFERENCES_KEY = 'oneapi.playground.preferences.v1';

const PROTOCOLS: Array<{ value: ProtocolName; label: string }> = [
  { value: 'chat', label: 'Chat' },
  { value: 'responses', label: 'Responses' },
  { value: 'anthropic', label: 'Anthropic' },
];

const DEFAULT_DRAFT: Draft = {
  protocol: 'chat',
  model: undefined,
  system: '',
  stream: true,
  maxTokens: 4096,
};

const EMPTY_SESSIONS: PlaygroundSessionSummary[] = [];
const EMPTY_MODELS: string[] = [];
const EMPTY_MESSAGES: ChatMessage[] = [];

function newId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function titleFromMessage(text: string): string {
  const firstLine = text.trim().split(/\r?\n/)[0] ?? '';
  const title = Array.from(firstLine).slice(0, 30).join('').trim();
  return title || '新对话';
}

function protocolLabel(protocol: ProtocolName): string {
  return PROTOCOLS.find((item) => item.value === protocol)?.label ?? protocol;
}

function loadPreferences(): Draft {
  try {
    const raw = localStorage.getItem(PREFERENCES_KEY);
    if (!raw) {
      return DEFAULT_DRAFT;
    }
    const parsed = JSON.parse(raw) as Partial<Draft>;
    return {
      protocol:
        parsed.protocol === 'responses' || parsed.protocol === 'anthropic'
          ? parsed.protocol
          : 'chat',
      model: typeof parsed.model === 'string' ? parsed.model : undefined,
      system: typeof parsed.system === 'string' ? parsed.system : '',
      stream: typeof parsed.stream === 'boolean' ? parsed.stream : true,
      maxTokens:
        typeof parsed.maxTokens === 'number' && parsed.maxTokens > 0
          ? parsed.maxTokens
          : 4096,
    };
  } catch {
    return DEFAULT_DRAFT;
  }
}

function toChatSession(session: PlaygroundSession): ChatSession {
  return {
    ...session,
    messages: session.messages.map((message) => ({
      ...message,
      uid: newId(),
      // A browser that closed mid-stream can leave a persisted "streaming"
      // status behind; nothing is running after loading, so show it as stopped.
      status: message.status === 'streaming' ? 'stopped' : message.status,
      tools: Array.isArray(message.tools) ? message.tools : [],
      meta: message.meta ?? {},
    })),
  };
}

function toPayload(session: ChatSession) {
  return {
    title: session.title,
    protocol: session.protocol,
    model: session.model,
    system: session.system,
    stream: session.stream,
    max_tokens: session.max_tokens,
    messages: session.messages.map((message) => ({
      role: message.role,
      content: message.content,
      reasoning: message.reasoning,
      tools: message.tools,
      status: message.status,
      error: message.error,
      meta: message.meta,
    })),
  };
}

function startOfDay(timestamp: number): number {
  const date = new Date(timestamp);
  date.setHours(0, 0, 0, 0);
  return date.getTime();
}

function groupSessions(items: PlaygroundSessionSummary[], now: number) {
  const day = startOfDay(now);
  const week = day - 6 * 86_400_000;
  const groups = new Map<string, PlaygroundSessionSummary[]>();
  for (const item of items) {
    const label =
      item.updated_at >= day
        ? '今天'
        : item.updated_at >= day - 86_400_000
          ? '昨天'
          : item.updated_at >= week
            ? '近 7 天'
            : '更早';
    const group = groups.get(label);
    if (group) {
      group.push(item);
    } else {
      groups.set(label, [item]);
    }
  }
  return Array.from(groups, ([label, sessions]) => ({ label, sessions }));
}

function formatReasoningDuration(ms: number | null | undefined): string {
  if (ms == null) {
    return '';
  }
  const seconds = ms / 1000;
  return seconds < 10 ? `${seconds.toFixed(1)} 秒` : `${Math.round(seconds)} 秒`;
}

function ProtocolSwitch({
  value,
  onChange,
  className,
}: {
  value: ProtocolName;
  onChange: (protocol: ProtocolName) => void;
  className?: string;
}) {
  return (
    <div
      className={cn(
        'flex items-center gap-0.5 rounded-lg bg-muted p-0.5',
        className,
      )}
    >
      {PROTOCOLS.map((protocol) => (
        <button
          key={protocol.value}
          type="button"
          onClick={() => onChange(protocol.value)}
          className={cn(
            'rounded-md px-2.5 py-1 text-xs font-medium transition-colors',
            value === protocol.value
              ? 'bg-background text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground',
          )}
        >
          {protocol.label}
        </button>
      ))}
    </div>
  );
}

function SessionSidebar({
  sessions,
  activeId,
  loading,
  onNew,
  onSelect,
  onDelete,
}: {
  sessions: PlaygroundSessionSummary[];
  activeId?: string;
  loading: boolean;
  onNew: () => void;
  onSelect: (id: string) => void;
  onDelete: (session: PlaygroundSessionSummary) => void;
}) {
  const now = useNow();
  const groups = groupSessions(sessions, now);

  return (
    <div className="flex h-full w-full flex-col">
      <div className="flex h-14 shrink-0 items-center border-b px-3">
        <Button
          type="button"
          variant="outline"
          className="w-full justify-start gap-2"
          onClick={onNew}
        >
          <Plus className="size-4" />
          新对话
        </Button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {loading ? (
          <div className="space-y-2 p-1">
            {[0, 1, 2, 3].map((item) => (
              <Skeleton key={item} className="h-12 w-full" />
            ))}
          </div>
        ) : groups.length === 0 ? (
          <div className="px-3 py-8 text-center text-xs text-muted-foreground">
            还没有历史会话
            <br />
            发送第一条消息后会保存在这里
          </div>
        ) : (
          groups.map((group) => (
            <div key={group.label} className="mb-3">
              <p className="px-2 pt-2 pb-1 text-[11px] font-medium text-muted-foreground">
                {group.label}
              </p>
              <div className="space-y-0.5">
                {group.sessions.map((item) => (
                  <div
                    key={item.id}
                    className={cn(
                      'group flex items-center gap-1 rounded-lg transition-colors',
                      activeId === item.id
                        ? 'bg-sidebar-accent text-sidebar-accent-foreground'
                        : 'hover:bg-sidebar-accent/60',
                    )}
                  >
                    <button
                      type="button"
                      onClick={() => onSelect(item.id)}
                      className="min-w-0 flex-1 px-3 py-2 text-left"
                    >
                      <span className="block truncate text-sm font-medium">
                        {item.title || '未命名会话'}
                      </span>
                      <span className="mt-0.5 block truncate text-[11px] text-muted-foreground">
                        {protocolLabel(item.protocol)} · {item.message_count} 条
                      </span>
                    </button>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      className="mr-1 shrink-0 opacity-100 sm:opacity-0 sm:group-hover:opacity-100 sm:focus-visible:opacity-100"
                      onClick={() => onDelete(item)}
                      title="删除会话"
                    >
                      <Trash2 className="size-3.5" />
                    </Button>
                  </div>
                ))}
              </div>
            </div>
          ))
        )}
      </div>

      <div className="shrink-0 border-t px-3 py-2 text-[11px] leading-relaxed text-muted-foreground">
        会话与思考过程保存在服务器
      </div>
    </div>
  );
}

function ReasoningBlock({
  reasoning,
  status,
  durationMs,
}: {
  reasoning: string;
  status: PlaygroundMessage['status'];
  durationMs?: number | null;
}) {
  const [open, setOpen] = useState(false);
  if (!reasoning) {
    return null;
  }
  const streaming = status === 'streaming';
  const expanded = streaming || open;

  return (
    <div className="overflow-hidden rounded-xl border bg-muted/30">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs text-muted-foreground transition-colors hover:text-foreground"
      >
        {streaming ? (
          <Loader2 className="size-3.5 animate-spin" />
        ) : (
          <Brain className="size-3.5" />
        )}
        <span>
          {streaming
            ? '思考中…'
            : `已深度思考${
                durationMs ? `（用时 ${formatReasoningDuration(durationMs)}）` : ''
              }`}
        </span>
        {streaming ? (
          <span className="ml-auto size-1.5 animate-pulse rounded-full bg-primary" />
        ) : (
          <ChevronDown
            className={cn(
              'ml-auto size-3.5 transition-transform',
              expanded && 'rotate-180',
            )}
          />
        )}
      </button>
      {expanded ? (
        <div className="max-h-80 overflow-y-auto border-t px-3 py-2 text-xs leading-6 whitespace-pre-wrap break-words text-muted-foreground">
          {reasoning}
        </div>
      ) : null}
    </div>
  );
}

function MessageMetaRow({ meta }: { meta: PlaygroundMessageMeta }) {
  const parts: string[] = [];
  if (meta.channel) {
    parts.push(`渠道 ${meta.channel}`);
  }
  if (meta.upstream_model) {
    parts.push(`上游 ${meta.upstream_model}`);
  }
  if (meta.usage?.total != null) {
    parts.push(`${formatTokens(meta.usage.total)} tokens`);
  }
  if (meta.elapsed_ms != null) {
    parts.push(`耗时 ${formatSeconds(meta.elapsed_ms)}`);
  }
  if (meta.first_token_ms != null) {
    parts.push(`首字 ${formatSeconds(meta.first_token_ms)}`);
  }
  if (meta.stop_reason) {
    parts.push(meta.stop_reason);
  }
  if (parts.length === 0) {
    return null;
  }
  return (
    <div className="flex flex-wrap gap-x-3 gap-y-1 text-[11px] text-muted-foreground">
      {parts.map((part) => (
        <span key={part}>{part}</span>
      ))}
    </div>
  );
}

function AssistantMessage({ message }: { message: ChatMessage }) {
  const [copied, setCopied] = useState(false);
  const streaming = message.status === 'streaming';
  const reasoningDuration = message.meta.reasoning_ms;

  async function copy() {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      toast.success('已复制');
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error('复制失败，请手动选择文本');
    }
  }

  return (
    <div className="flex gap-3">
      <div className="mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary">
        <Sparkles className="size-4" />
      </div>
      <div className="min-w-0 flex-1 space-y-2.5">
        <ReasoningBlock
          reasoning={message.reasoning}
          status={message.status}
          durationMs={reasoningDuration}
        />

        {message.error ? (
          <Alert variant="destructive">
            <AlertTitle>请求失败</AlertTitle>
            <AlertDescription className="break-words">
              {message.error}
            </AlertDescription>
          </Alert>
        ) : null}

        {message.content ? (
          <div className="relative">
            <Markdown>{message.content}</Markdown>
            {streaming ? (
              <span className="mt-1 inline-block h-4 w-[3px] animate-pulse rounded-full bg-foreground align-middle" />
            ) : null}
          </div>
        ) : streaming ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="size-3.5 animate-spin" />
            正在生成…
          </div>
        ) : null}

        {message.tools.length > 0 ? (
          <details className="rounded-xl border bg-muted/20">
            <summary className="cursor-pointer px-3 py-2 text-xs font-medium text-muted-foreground select-none hover:text-foreground">
              工具调用（{message.tools.length}）
            </summary>
            <div className="space-y-2 border-t p-2">
              {message.tools.map((tool) => (
                <div
                  key={tool.key}
                  className="space-y-1.5 rounded-lg border bg-background p-2"
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <Badge variant="outline">{tool.name || 'function'}</Badge>
                    <span className="truncate font-mono text-[11px] text-muted-foreground">
                      {tool.id || '-'}
                    </span>
                  </div>
                  <CodeBlock showCopy={false}>
                    {prettyArguments(tool.arguments) || '（无参数）'}
                  </CodeBlock>
                </div>
              ))}
            </div>
          </details>
        ) : null}

        <div className="flex flex-wrap items-center gap-2">
          <MessageMetaRow meta={message.meta} />
          {message.status === 'stopped' ? (
            <span className="text-[11px] text-amber-600 dark:text-amber-400">
              已停止
            </span>
          ) : null}
          {!streaming && message.content ? (
            <Button
              type="button"
              variant="ghost"
              size="icon-sm"
              className="-my-1 size-6 text-muted-foreground"
              onClick={() => void copy()}
              title="复制回答"
            >
              {copied ? (
                <Check className="size-3.5" />
              ) : (
                <Copy className="size-3.5" />
              )}
            </Button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function EmptyConversation() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center py-16 text-center">
      <div className="mb-4 flex size-12 items-center justify-center rounded-2xl bg-primary/10 text-primary">
        <Sparkles className="size-6" />
      </div>
      <h2 className="text-lg font-semibold">有什么可以帮你的？</h2>
      <p className="mt-1 max-w-sm text-sm leading-relaxed text-muted-foreground">
        选择协议与模型后直接输入消息。思考过程和回答会实时流式展示，完成后保存在左侧历史中。
      </p>
    </div>
  );
}

function LoadingConversation() {
  return (
    <div className="w-full space-y-6 py-4">
      <Skeleton className="ml-auto h-10 w-2/5" />
      <div className="flex gap-3">
        <Skeleton className="size-8 shrink-0 rounded-lg" />
        <div className="flex-1 space-y-2">
          <Skeleton className="h-4 w-1/3" />
          <Skeleton className="h-4 w-full" />
          <Skeleton className="h-4 w-4/5" />
        </div>
      </div>
    </div>
  );
}

function Composer({
  value,
  running,
  disabled,
  onChange,
  onSend,
  onStop,
}: {
  value: string;
  running: boolean;
  disabled: boolean;
  onChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
}) {
  function handleKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key !== 'Enter' || event.shiftKey) {
      return;
    }
    if (event.nativeEvent.isComposing) {
      return;
    }
    event.preventDefault();
    onSend();
  }

  return (
    <div className="shrink-0 border-t bg-background/80 px-3 pt-3 pb-3 backdrop-blur sm:px-4">
      <div className="mx-auto w-full max-w-3xl">
        <div className="relative rounded-2xl border bg-background shadow-sm transition-shadow focus-within:ring-2 focus-within:ring-ring/40">
          <Textarea
            value={value}
            onChange={(event) => onChange(event.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="给模型发送消息…"
            rows={2}
            className="max-h-48 resize-none border-0 bg-transparent py-3 pr-14 shadow-none focus-visible:ring-0"
          />
          <div className="absolute right-2 bottom-2">
            {running ? (
              <Button
                type="button"
                size="icon"
                variant="secondary"
                onClick={onStop}
                title="停止生成"
              >
                <Square className="size-4" />
              </Button>
            ) : (
              <Button
                type="button"
                size="icon"
                onClick={onSend}
                disabled={disabled}
                title="发送"
              >
                <Send className="size-4" />
              </Button>
            )}
          </div>
        </div>
        <p className="mt-2 text-center text-[11px] text-muted-foreground">
          Enter 发送，Shift + Enter 换行 · 请求会计入日志
        </p>
      </div>
    </div>
  );
}

export default function Playground() {
  const queryClient = useQueryClient();
  const modelsQuery = useQuery({
    queryKey: ['models'],
    queryFn: () => api.get<ModelsOutput>('/api/models'),
  });
  const sessionsQuery = useQuery({
    queryKey: ['playground-sessions'],
    queryFn: () =>
      api.get<PlaygroundSessionsOutput>('/api/playground/sessions'),
  });

  const [draft, setDraft] = useState<Draft>(loadPreferences);
  const [session, setSession] = useState<ChatSession | null>(null);
  const [input, setInput] = useState('');
  const [running, setRunning] = useState(false);
  const [loadingSessionId, setLoadingSessionId] = useState<string | null>(null);
  const [mobileOpen, setMobileOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [pendingDelete, setPendingDelete] =
    useState<PlaygroundSessionSummary | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [stickToBottom, setStickToBottom] = useState(true);

  const abortRef = useRef<AbortController | null>(null);
  const runStateRef = useRef<RunState | null>(null);
  const sessionRef = useRef<ChatSession | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const openRequestRef = useRef(0);
  const deletedIdsRef = useRef(new Set<string>());

  const effectiveModel =
    draft.model?.trim() || modelsQuery.data?.models[0] || '';
  const messages = session?.messages ?? EMPTY_MESSAGES;

  const persistSession = useCallback(
    async (
      value: ChatSession,
      options?: { notify?: boolean; invalidate?: boolean },
    ) => {
      if (value.messages.length === 0 || deletedIdsRef.current.has(value.id)) {
        return;
      }
      try {
        await api.put<PlaygroundSessionOutput>(
          `/api/playground/sessions/${value.id}`,
          toPayload(value),
        );
        if (options?.invalidate !== false) {
          void queryClient.invalidateQueries({
            queryKey: ['playground-sessions'],
          });
        }
      } catch (error) {
        if (options?.notify) {
          toast.error(errorMessage(error));
        }
      }
    },
    [queryClient],
  );

  // The autosave timer and the final save both read the last committed session
  // from this ref, so they never persist a half-rendered snapshot.
  useEffect(() => {
    sessionRef.current = session;
  }, [session]);

  useEffect(() => {
    try {
      localStorage.setItem(PREFERENCES_KEY, JSON.stringify(draft));
    } catch {
      // Preferences are optional; private mode can reject writes.
    }
  }, [draft]);

  useEffect(() => {
    const element = scrollRef.current;
    if (element && stickToBottom) {
      element.scrollTop = element.scrollHeight;
    }
  }, [session, stickToBottom]);

  // Throttle autosaves from the first change instead of debouncing every token:
  // a long stream still reaches SQLite roughly every 1.2 seconds.
  useEffect(() => {
    if (!session || session.messages.length === 0) {
      return;
    }
    if (saveTimerRef.current !== null) {
      return;
    }
    saveTimerRef.current = window.setTimeout(() => {
      saveTimerRef.current = null;
      const current = sessionRef.current;
      if (current) {
        void persistSession(current, { invalidate: false });
      }
    }, 1200);
  }, [session, persistSession]);

  const wasRunningRef = useRef(false);
  useEffect(() => {
    if (wasRunningRef.current && !running) {
      const current = sessionRef.current;
      if (current) {
        void persistSession(current, { notify: true });
      }
    }
    wasRunningRef.current = running;
  }, [running, persistSession]);

  useEffect(
    () => () => {
      abortRef.current?.abort();
      if (saveTimerRef.current !== null) {
        window.clearTimeout(saveTimerRef.current);
      }
    },
    [],
  );

  function updateDraft(patch: Partial<Draft>) {
    const next: Draft = { ...draft, ...patch };
    setDraft(next);
    setSession((current) =>
      current
        ? {
            ...current,
            protocol: next.protocol,
            // An empty draft model falls back to the first catalog entry in the
            // UI; keep the last non-empty value so autosave stays valid.
            model: next.model?.trim() ? next.model : current.model,
            system: next.system,
            stream: next.stream,
            max_tokens: next.maxTokens,
          }
        : current,
    );
  }

  function applyTurn(
    sessionId: string,
    uid: string,
    patch: Partial<ChatMessage>,
    updatedAt: number,
  ) {
    setSession((current) => {
      if (!current || current.id !== sessionId) {
        return current;
      }
      return {
        ...current,
        updated_at: updatedAt,
        messages: current.messages.map((message) =>
          message.uid === uid ? { ...message, ...patch } : message,
        ),
      };
    });
  }

  function markRunStopped() {
    const run = runStateRef.current;
    abortRef.current?.abort();
    abortRef.current = null;
    runStateRef.current = null;
    setRunning(false);
    if (!run) {
      return;
    }
    const current = sessionRef.current;
    if (!current || current.id !== run.sessionId) {
      return;
    }
    const next: ChatSession = {
      ...current,
      messages: current.messages.map((message) =>
        message.uid === run.uid && message.status === 'streaming'
          ? { ...message, status: 'stopped' }
          : message,
      ),
    };
    sessionRef.current = next;
    setSession(next);
    void persistSession(next);
  }

  async function runStream(
    conversation: ChatSession,
    assistantUid: string,
    token: string,
  ) {
    const controller = new AbortController();
    abortRef.current = controller;
    const started = performance.now();
    let firstTokenMs: number | null = null;
    const acc: TurnAccumulator = emptyAccumulator();
    let reasoningStartedAt: number | null = null;

    function buildMeta(
      elapsedMs: number | null,
      reasoningMs: number | null,
    ): PlaygroundMessageMeta {
      return {
        channel: acc.channel,
        upstream_model: acc.upstreamModel,
        model: acc.model,
        response_id: acc.responseId,
        stop_reason: acc.stopReason,
        usage: acc.usage,
        elapsed_ms: elapsedMs,
        first_token_ms: firstTokenMs,
        reasoning_ms: reasoningMs,
      };
    }

    function snapshot(): Partial<ChatMessage> {
      return {
        content: acc.text,
        reasoning: acc.reasoning,
        tools: [...acc.tools],
        meta: buildMeta(null, null),
      };
    }

    try {
      const response = await requestRaw('/api/playground', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(
          buildPlaygroundRequest(
            {
              protocol: conversation.protocol,
              model: conversation.model,
              system: conversation.system,
              stream: conversation.stream,
              maxTokens: conversation.max_tokens,
            },
            conversation.messages.filter(
              (message) => message.uid !== assistantUid,
            ),
          ),
        ),
        signal: controller.signal,
      });

      acc.channel = decodeRouteHeader(
        response.headers.get('x-oneapi-channel'),
      );
      acc.upstreamModel = decodeRouteHeader(
        response.headers.get('x-oneapi-upstream-model'),
      );

      if (conversation.stream) {
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
          applyEvent(conversation.protocol, event.event, payload, acc);
          if (reasoningStartedAt === null && acc.reasoning.length > 0) {
            reasoningStartedAt = performance.now();
          }
          if (
            firstTokenMs === null &&
            (acc.text || acc.reasoning || acc.tools.length > 0)
          ) {
            firstTokenMs = Math.round(performance.now() - started);
          }
          applyTurn(conversation.id, assistantUid, snapshot(), Date.now());
        }
      } else {
        const payload = (await response.json()) as Record<string, unknown>;
        applyComplete(conversation.protocol, payload, acc);
        if (reasoningStartedAt === null && acc.reasoning.length > 0) {
          reasoningStartedAt = started;
        }
        if (
          firstTokenMs === null &&
          (acc.text || acc.reasoning || acc.tools.length > 0)
        ) {
          firstTokenMs = Math.round(performance.now() - started);
        }
        applyTurn(conversation.id, assistantUid, snapshot(), Date.now());
      }
    } catch (error) {
      if (!controller.signal.aborted) {
        acc.error = errorMessage(error);
      }
    } finally {
      if (abortRef.current === controller) {
        abortRef.current = null;
      }
      const elapsedMs = Math.round(performance.now() - started);
      const reasoningMs =
        reasoningStartedAt === null
          ? null
          : Math.round(performance.now() - reasoningStartedAt);
      const status: PlaygroundMessage['status'] = controller.signal.aborted
        ? 'stopped'
        : acc.error
          ? 'error'
          : 'done';
      applyTurn(
        conversation.id,
        assistantUid,
        {
          content: acc.text,
          reasoning: acc.reasoning,
          tools: [...acc.tools],
          status,
          error: acc.error ?? '',
          meta: buildMeta(elapsedMs, reasoningMs),
        },
        Date.now(),
      );
      if (runStateRef.current?.token === token) {
        runStateRef.current = null;
        setRunning(false);
      }
    }
  }

  async function send() {
    const text = input.trim();
    if (!text || running) {
      return;
    }
    const model = effectiveModel;
    if (!model) {
      toast.error('请先选择或输入模型');
      return;
    }

    const now = Date.now();
    const base: ChatSession =
      session ?? {
        id: newId(),
        title: '',
        protocol: draft.protocol,
        model,
        system: draft.system,
        stream: draft.stream,
        max_tokens: draft.maxTokens,
        created_at: now,
        updated_at: now,
        messages: [],
      };
    const userMessage: ChatMessage = {
      uid: newId(),
      role: 'user',
      content: text,
      reasoning: '',
      tools: [],
      status: 'done',
      error: '',
      meta: {},
    };
    const assistantMessage: ChatMessage = {
      uid: newId(),
      role: 'assistant',
      content: '',
      reasoning: '',
      tools: [],
      status: 'streaming',
      error: '',
      meta: {},
    };
    const next: ChatSession = {
      ...base,
      title: base.messages.length === 0 ? titleFromMessage(text) : base.title,
      protocol: draft.protocol,
      model,
      system: draft.system,
      stream: draft.stream,
      max_tokens: draft.maxTokens,
      updated_at: now,
      messages: [...base.messages, userMessage, assistantMessage],
    };

    sessionRef.current = next;
    setSession(next);
    setInput('');
    setStickToBottom(true);
    setRunning(true);
    setMobileOpen(false);

    // Register the run before the first save so pressing stop during the
    // network round-trip can still mark the assistant placeholder stopped.
    const runToken = newId();
    runStateRef.current = {
      token: runToken,
      sessionId: next.id,
      uid: assistantMessage.uid,
    };
    await persistSession(next, { notify: true });
    if (runStateRef.current?.token !== runToken) {
      return;
    }
    await runStream(next, assistantMessage.uid, runToken);
  }

  function startNewChat() {
    if (runStateRef.current) {
      markRunStopped();
    }
    openRequestRef.current += 1;
    setLoadingSessionId(null);
    sessionRef.current = null;
    setSession(null);
    setInput('');
    setMobileOpen(false);
    setStickToBottom(true);
  }

  async function openSession(id: string) {
    if (runStateRef.current) {
      markRunStopped();
    }
    const requestId = openRequestRef.current + 1;
    openRequestRef.current = requestId;
    setMobileOpen(false);
    setLoadingSessionId(id);
    setStickToBottom(true);
    try {
      const data = await api.get<PlaygroundSessionOutput>(
        `/api/playground/sessions/${id}`,
      );
      if (openRequestRef.current !== requestId) {
        return;
      }
      const loaded = toChatSession(data.session);
      sessionRef.current = loaded;
      setSession(loaded);
      setDraft({
        protocol: loaded.protocol,
        model: loaded.model,
        system: loaded.system,
        stream: loaded.stream,
        maxTokens: loaded.max_tokens,
      });
    } catch (error) {
      if (openRequestRef.current === requestId) {
        toast.error(errorMessage(error));
      }
    } finally {
      if (openRequestRef.current === requestId) {
        setLoadingSessionId(null);
      }
    }
  }

  async function confirmDelete() {
    if (!pendingDelete) {
      return;
    }
    const target = pendingDelete;
    setDeleting(true);
    deletedIdsRef.current.add(target.id);
    if (runStateRef.current?.sessionId === target.id) {
      abortRef.current?.abort();
      abortRef.current = null;
      runStateRef.current = null;
      setRunning(false);
    }
    if (sessionRef.current?.id === target.id) {
      openRequestRef.current += 1;
      sessionRef.current = null;
      setSession(null);
      setLoadingSessionId(null);
    }
    try {
      await api.delete<{ ok: boolean }>(
        `/api/playground/sessions/${target.id}`,
      );
      await queryClient.invalidateQueries({
        queryKey: ['playground-sessions'],
      });
      toast.success('已删除会话');
      setPendingDelete(null);
    } catch (error) {
      deletedIdsRef.current.delete(target.id);
      toast.error(errorMessage(error));
    } finally {
      setDeleting(false);
    }
  }

  function handleScroll() {
    const element = scrollRef.current;
    if (!element) {
      return;
    }
    const distance =
      element.scrollHeight - element.scrollTop - element.clientHeight;
    setStickToBottom(distance < 80);
  }

  const sessions = sessionsQuery.data?.items ?? EMPTY_SESSIONS;
  const title = session?.title || '新对话';

  return (
    <div className="flex h-[calc(100dvh-3.5rem)] min-h-0 overflow-hidden bg-background">
      <aside className="hidden w-64 shrink-0 border-r bg-sidebar lg:flex">
        <SessionSidebar
          sessions={sessions}
          activeId={session?.id}
          loading={sessionsQuery.isLoading}
          onNew={startNewChat}
          onSelect={(id) => void openSession(id)}
          onDelete={setPendingDelete}
        />
      </aside>

      <Sheet open={mobileOpen} onOpenChange={setMobileOpen}>
        <SheetContent
          side="left"
          className="w-72 p-0 data-[side=left]:w-72 data-[side=left]:sm:max-w-72"
        >
          <SheetHeader className="sr-only">
            <SheetTitle>历史会话</SheetTitle>
          </SheetHeader>
          <SessionSidebar
            sessions={sessions}
            activeId={session?.id}
            loading={sessionsQuery.isLoading}
            onNew={startNewChat}
            onSelect={(id) => void openSession(id)}
            onDelete={setPendingDelete}
          />
        </SheetContent>
      </Sheet>

      <section className="flex min-w-0 flex-1 flex-col">
        <header className="flex h-14 shrink-0 items-center gap-2 border-b px-3 sm:px-4">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="lg:hidden"
            onClick={() => setMobileOpen(true)}
            title="打开历史会话"
          >
            <Menu />
          </Button>

          <div className="min-w-0 flex-1">
            <h1 className="truncate text-sm font-semibold">{title}</h1>
            <p className="truncate text-[11px] text-muted-foreground">
              {protocolLabel(draft.protocol)} · {effectiveModel || '未选择模型'}
            </p>
          </div>

          <ProtocolSwitch
            value={draft.protocol}
            onChange={(protocol) => updateDraft({ protocol })}
            className="hidden md:flex"
          />

          <Input
            list="playground-models"
            value={effectiveModel}
            onChange={(event) => updateDraft({ model: event.target.value })}
            placeholder="模型"
            className="hidden w-48 font-mono sm:block"
            aria-label="模型"
          />
          <datalist id="playground-models">
            {(modelsQuery.data?.models ?? EMPTY_MODELS).map((item) => (
              <option key={item} value={item} />
            ))}
          </datalist>

          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => setSettingsOpen(true)}
            title="对话设置"
          >
            <Settings2 />
          </Button>
        </header>

        <div
          ref={scrollRef}
          onScroll={handleScroll}
          className="min-h-0 flex-1 overflow-y-auto"
        >
          <div className="mx-auto flex min-h-full w-full max-w-3xl flex-col px-4 py-6">
            {loadingSessionId ? (
              <LoadingConversation />
            ) : messages.length > 0 ? (
              <div className="space-y-7">
                {messages.map((message) =>
                  message.role === 'user' ? (
                    <div key={message.uid} className="flex justify-end">
                      <div className="max-w-[85%] rounded-2xl bg-muted px-4 py-3 text-sm leading-7 break-words whitespace-pre-wrap">
                        {message.content}
                      </div>
                    </div>
                  ) : (
                    <AssistantMessage key={message.uid} message={message} />
                  ),
                )}
              </div>
            ) : (
              <EmptyConversation />
            )}
          </div>
        </div>

        <Composer
          value={input}
          running={running}
          disabled={!input.trim() || !effectiveModel}
          onChange={setInput}
          onSend={() => void send()}
          onStop={markRunStopped}
        />
      </section>

      <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>对话设置</DialogTitle>
            <DialogDescription>
              配置随当前会话保存，新会话沿用上次选择。
            </DialogDescription>
          </DialogHeader>
          <div className="space-y-4">
            <Field label="协议">
              <ProtocolSwitch
                value={draft.protocol}
                onChange={(protocol) => updateDraft({ protocol })}
              />
            </Field>

            <Field label="模型" htmlFor="playground-model">
              <Input
                id="playground-model"
                list="playground-models"
                value={effectiveModel}
                onChange={(event) => updateDraft({ model: event.target.value })}
                placeholder="选择或输入模型"
                className="font-mono"
              />
            </Field>

            <Field
              label="System / instructions（可选）"
              htmlFor="playground-system"
            >
              <Textarea
                id="playground-system"
                value={draft.system}
                onChange={(event) => updateDraft({ system: event.target.value })}
                placeholder="你是一个乐于助人的助手。"
                rows={5}
                className="max-h-60"
              />
            </Field>

            <div className="flex flex-wrap items-center gap-6">
              <div className="flex items-center gap-2">
                <Switch
                  id="playground-stream"
                  checked={draft.stream}
                  onCheckedChange={(stream) => updateDraft({ stream })}
                />
                <label
                  htmlFor="playground-stream"
                  className="text-sm font-medium"
                >
                  流式输出
                </label>
              </div>
              <div className="flex items-center gap-2">
                <label
                  htmlFor="playground-max-tokens"
                  className="text-sm font-medium"
                >
                  max_tokens
                </label>
                <Input
                  id="playground-max-tokens"
                  type="number"
                  min={1}
                  max={200_000}
                  value={draft.maxTokens}
                  onChange={(event) =>
                    updateDraft({
                      maxTokens: Number(event.target.value) || 4096,
                    })
                  }
                  className="w-28"
                />
              </div>
            </div>

            <p className="text-xs leading-relaxed text-muted-foreground">
              max_tokens 仅用于 Anthropic 协议；Chat/Responses 使用服务端默认值。
            </p>
          </div>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={pendingDelete !== null}
        onOpenChange={(open) => {
          if (!open) {
            setPendingDelete(null);
          }
        }}
        title="删除这个会话？"
        description={
          pendingDelete
            ? `“${pendingDelete.title || '未命名会话'}”及其中的思考过程会从服务器永久删除。`
            : undefined
        }
        confirmLabel="删除"
        destructive
        loading={deleting}
        onConfirm={() => void confirmDelete()}
      />
    </div>
  );
}
