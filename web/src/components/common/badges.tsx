import { Badge } from '@/components/ui/badge';
import { useNow } from '@/hooks/useNow';
import { cn } from '@/lib/utils';

const protocolStyles: Record<string, string> = {
  chat: 'border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300',
  responses:
    'border-violet-500/30 bg-violet-500/10 text-violet-700 dark:text-violet-300',
  anthropic:
    'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300',
  openai_chat: 'border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300',
  openai_responses:
    'border-violet-500/30 bg-violet-500/10 text-violet-700 dark:text-violet-300',
};

const protocolLabels: Record<string, string> = {
  chat: 'Chat',
  responses: 'Responses',
  anthropic: 'Anthropic',
  openai_chat: 'OpenAI Chat',
  openai_responses: 'OpenAI Responses',
};

export function ProtocolBadge({ protocol }: { protocol: string }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        'font-normal',
        protocolStyles[protocol] ?? 'text-muted-foreground',
      )}
    >
      {protocolLabels[protocol] ?? protocol}
    </Badge>
  );
}

export function StatusCodeBadge({ code }: { code: number }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        'font-mono font-normal',
        code >= 400
          ? 'border-destructive/30 bg-destructive/10 text-destructive'
          : 'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300',
      )}
    >
      {code}
    </Badge>
  );
}

export function EnabledBadge({
  enabled,
  disabledUntil,
}: {
  enabled: boolean;
  disabledUntil?: number | null;
}) {
  const now = useNow();
  const cooling = enabled && disabledUntil != null && disabledUntil > now;

  return (
    <Badge
      variant="outline"
      className={cn(
        'font-normal',
        !enabled && 'text-muted-foreground',
        cooling &&
          'border-amber-500/30 bg-amber-500/10 text-amber-700 dark:text-amber-300',
        enabled &&
          !cooling &&
          'border-emerald-500/30 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300',
      )}
    >
      {!enabled ? '停用' : cooling ? '冷却中' : '启用'}
    </Badge>
  );
}

export function SourceBadge({ source }: { source: 'gateway' | 'playground' }) {
  return source === 'playground' ? (
    <Badge
      variant="outline"
      className="border-violet-500/30 bg-violet-500/10 font-normal text-violet-700 dark:text-violet-300"
    >
      Playground
    </Badge>
  ) : (
    <Badge variant="outline" className="font-normal text-muted-foreground">
      网关
    </Badge>
  );
}

export function StreamBadge({ stream }: { stream: boolean }) {
  return (
    <Badge
      variant="outline"
      className={cn(
        'font-normal',
        stream
          ? 'border-sky-500/30 bg-sky-500/10 text-sky-700 dark:text-sky-300'
          : 'text-muted-foreground',
      )}
    >
      {stream ? '流式' : '非流式'}
    </Badge>
  );
}
