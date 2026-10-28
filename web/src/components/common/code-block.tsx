import { Check, Copy } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';

interface CodeBlockProps {
  children: string;
  className?: string;
  copyText?: string;
  showCopy?: boolean;
}

export function CodeBlock({
  children,
  className,
  copyText,
  showCopy = true,
}: CodeBlockProps) {
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(copyText ?? children);
      setCopied(true);
      toast.success('已复制');
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error('复制失败，请手动选择文本');
    }
  }

  return (
    <div className={cn('relative min-w-0', className)}>
      {showCopy ? (
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="absolute right-2 top-2 z-10 bg-background/80 backdrop-blur"
          onClick={() => void copy()}
          title="复制"
        >
          {copied ? (
            <Check className="size-3.5" />
          ) : (
            <Copy className="size-3.5" />
          )}
        </Button>
      ) : null}
      <pre className="max-h-[360px] overflow-auto rounded-lg border bg-muted/40 p-3 pr-12 font-mono text-xs leading-relaxed whitespace-pre-wrap break-words">
        {children}
      </pre>
    </div>
  );
}
