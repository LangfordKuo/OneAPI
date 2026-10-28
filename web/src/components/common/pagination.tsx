import { ChevronLeft, ChevronRight } from 'lucide-react';
import { Button } from '@/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';

interface PaginationProps {
  page: number;
  pageSize: number;
  total: number;
  pageSizeOptions?: number[];
  onPageChange: (page: number) => void;
  onPageSizeChange: (pageSize: number) => void;
}

export function Pagination({
  page,
  pageSize,
  total,
  pageSizeOptions = [20, 50, 100],
  onPageChange,
  onPageSizeChange,
}: PaginationProps) {
  const pages = Math.max(1, Math.ceil(total / pageSize));

  return (
    <div className="flex flex-col gap-3 border-t px-1 pt-4 sm:flex-row sm:items-center sm:justify-between">
      <p className="text-xs text-muted-foreground">
        共 {total.toLocaleString('zh-CN')} 条
      </p>
      <div className="flex items-center gap-3">
        <Select
          items={pageSizeOptions.map((size) => ({
            value: String(size),
            label: `${size} 条/页`,
          }))}
          value={String(pageSize)}
          onValueChange={(value) => onPageSizeChange(Number(value))}
        >
          <SelectTrigger size="sm" className="w-[110px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {pageSizeOptions.map((size) => (
              <SelectItem key={size} value={String(size)}>
                {size} 条/页
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <span className="text-xs text-muted-foreground">
          第 {page} / {pages} 页
        </span>
        <div className="flex gap-1">
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            disabled={page <= 1}
            onClick={() => onPageChange(page - 1)}
            title="上一页"
          >
            <ChevronLeft />
          </Button>
          <Button
            type="button"
            variant="outline"
            size="icon-sm"
            disabled={page >= pages}
            onClick={() => onPageChange(page + 1)}
            title="下一页"
          >
            <ChevronRight />
          </Button>
        </div>
      </div>
    </div>
  );
}
