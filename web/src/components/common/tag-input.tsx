import { X } from 'lucide-react';
import { useState } from 'react';
import { Badge } from '@/components/ui/badge';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

interface TagInputProps {
  value: string[];
  onChange: (value: string[]) => void;
  placeholder?: string;
}

export function TagInput({ value, onChange, placeholder }: TagInputProps) {
  const [draft, setDraft] = useState('');

  function commit(raw: string) {
    const parts = raw
      .split(/[,，\n]/)
      .map((part) => part.trim())
      .filter(Boolean);
    if (parts.length === 0) {
      return;
    }

    const next = [...value];
    for (const part of parts) {
      if (!next.includes(part)) {
        next.push(part);
      }
    }
    onChange(next);
    setDraft('');
  }

  return (
    <div className="space-y-2">
      <div className="flex gap-2">
        <Input
          value={draft}
          placeholder={placeholder}
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' || event.key === ',') {
              event.preventDefault();
              commit(draft);
            } else if (event.key === 'Backspace' && !draft && value.length > 0) {
              onChange(value.slice(0, -1));
            }
          }}
          onBlur={() => commit(draft)}
        />
        <Button
          type="button"
          variant="outline"
          onClick={() => commit(draft)}
          disabled={!draft.trim()}
        >
          添加
        </Button>
      </div>
      {value.length > 0 ? (
        <div className="flex flex-wrap gap-1.5">
          {value.map((tag) => (
            <Badge
              key={tag}
              variant="secondary"
              className="gap-1 pr-1 font-mono text-xs font-normal"
            >
              {tag}
              <button
                type="button"
                className="rounded-sm p-0.5 hover:bg-background/60"
                onClick={() => onChange(value.filter((item) => item !== tag))}
                title="移除"
              >
                <X className="size-3" />
              </button>
            </Badge>
          ))}
        </div>
      ) : null}
    </div>
  );
}
