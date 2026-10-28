import { Plus, Trash2 } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

export interface KeyValueRow {
  key: string;
  value: string;
}

interface KeyValueEditorProps {
  value: KeyValueRow[];
  onChange: (value: KeyValueRow[]) => void;
  keyPlaceholder?: string;
  valuePlaceholder?: string;
}

export function KeyValueEditor({
  value,
  onChange,
  keyPlaceholder = '键',
  valuePlaceholder = '值',
}: KeyValueEditorProps) {
  return (
    <div className="space-y-2">
      {value.map((row, index) => (
        <div key={index} className="flex items-start gap-2">
          <Input
            value={row.key}
            placeholder={keyPlaceholder}
            onChange={(event) => {
              const next = [...value];
              next[index] = { ...row, key: event.target.value };
              onChange(next);
            }}
          />
          <Input
            value={row.value}
            placeholder={valuePlaceholder}
            onChange={(event) => {
              const next = [...value];
              next[index] = { ...row, value: event.target.value };
              onChange(next);
            }}
          />
          <Button
            type="button"
            variant="ghost"
            size="icon"
            onClick={() => onChange(value.filter((_, i) => i !== index))}
            title="删除这一行"
          >
            <Trash2 className="size-4" />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => onChange([...value, { key: '', value: '' }])}
      >
        <Plus className="size-3.5" />
        添加一行
      </Button>
    </div>
  );
}
