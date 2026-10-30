import { memo } from 'react';
import dayjs from 'dayjs';
import {
  Area,
  AreaChart,
  Bar,
  BarChart,
  CartesianGrid,
  Cell,
  Pie,
  PieChart,
  XAxis,
  YAxis,
} from 'recharts';
import type { BreakdownItem, TimeseriesPoint } from '@/api/types';
import { EmptyState } from '@/components/common/empty-state';
import { formatTokens } from '@/lib/format';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/components/ui/card';
import {
  ChartContainer,
  ChartTooltip,
  ChartTooltipContent,
  type ChartConfig,
} from '@/components/ui/chart';

interface Props {
  points: TimeseriesPoint[];
  models: BreakdownItem[];
  channels: BreakdownItem[];
}

const requestConfig = {
  requests: { label: '请求', color: 'var(--chart-1)' },
  errors: { label: '错误', color: 'var(--chart-5)' },
} satisfies ChartConfig;

const tokenConfig = {
  tokens: { label: 'Tokens', color: 'var(--chart-2)' },
} satisfies ChartConfig;

const pieConfig = {
  value: { label: '请求' },
} satisfies ChartConfig;

const PALETTE = [
  'var(--chart-1)',
  'var(--chart-2)',
  'var(--chart-3)',
  'var(--chart-4)',
  'var(--chart-5)',
  'var(--chart-1)',
  'var(--chart-2)',
  'var(--chart-3)',
];

function timeLabel(timestamp: number): string {
  return dayjs(timestamp).format('MM-DD HH:mm');
}

function PieLegend({
  items,
}: {
  items: Array<{ name: string; value: number }>;
}) {
  if (items.length === 0) {
    return null;
  }

  return (
    <div className="mt-3 flex flex-wrap gap-x-4 gap-y-1.5">
      {items.slice(0, 8).map((item, index) => (
        <div
          key={`${item.name}-${index}`}
          className="flex items-center gap-1.5 text-xs text-muted-foreground"
        >
          <span
            className="size-2 shrink-0 rounded-[2px]"
            style={{ backgroundColor: PALETTE[index % PALETTE.length] }}
          />
          <span
            className="max-w-[140px] truncate"
            title={item.name}
          >
            {item.name}
          </span>
          <span className="font-mono tabular-nums text-foreground">
            {item.value.toLocaleString('zh-CN')}
          </span>
        </div>
      ))}
      {items.length > 8 ? (
        <span className="text-xs text-muted-foreground">
          +{items.length - 8} 项
        </span>
      ) : null}
    </div>
  );
}

interface PieItem {
  name: string;
  value: number;
}

/** One donut card for a model/channel breakdown. */
function BreakdownPie({
  title,
  empty,
  items,
}: {
  title: string;
  empty: string;
  items: PieItem[];
}) {
  return (
    <Card>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
      </CardHeader>
      <CardContent>
        {items.length === 0 ? (
          <EmptyState title={empty} />
        ) : (
          <>
            <ChartContainer
              config={pieConfig}
              className="mx-auto aspect-auto h-[240px] w-full"
            >
              <PieChart>
                <ChartTooltip content={<ChartTooltipContent hideLabel />} />
                <Pie
                  data={items}
                  dataKey="value"
                  nameKey="name"
                  innerRadius={58}
                  outerRadius={90}
                  paddingAngle={2}
                  strokeWidth={0}
                  isAnimationActive={false}
                >
                  {items.map((entry, index) => (
                    <Cell
                      key={entry.name}
                      fill={PALETTE[index % PALETTE.length]}
                    />
                  ))}
                </Pie>
              </PieChart>
            </ChartContainer>
            <PieLegend items={items} />
          </>
        )}
      </CardContent>
    </Card>
  );
}

function DashboardCharts({ points, models, channels }: Props) {
  const requestData = points.map((point) => ({
    time: timeLabel(point.timestamp),
    requests: point.requests,
    errors: point.errors,
  }));
  const tokenData = points.map((point) => ({
    time: timeLabel(point.timestamp),
    tokens: point.total_tokens,
  }));
  const modelData = models.map((item) => ({
    name: item.label,
    value: item.requests,
  }));
  const channelData = channels.map((item) => ({
    name: item.label,
    value: item.requests,
  }));

  return (
    <div className="grid gap-4 lg:grid-cols-2">
      <Card>
        <CardHeader>
          <CardTitle>请求 / 错误时序</CardTitle>
        </CardHeader>
        <CardContent>
          {requestData.length === 0 ? (
            <EmptyState title="当前范围没有请求数据" />
          ) : (
            <ChartContainer
              config={requestConfig}
              className="aspect-auto h-[260px] w-full"
            >
              <AreaChart
                data={requestData}
                margin={{ left: 0, right: 8, top: 8, bottom: 0 }}
              >
                <CartesianGrid vertical={false} />
                <XAxis
                  dataKey="time"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  minTickGap={24}
                />
                <YAxis
                  tickLine={false}
                  axisLine={false}
                  width={36}
                  allowDecimals={false}
                />
                <ChartTooltip
                  cursor={false}
                  content={<ChartTooltipContent indicator="line" />}
                />
                <Area
                  dataKey="requests"
                  type="monotone"
                  fill="var(--color-requests)"
                  fillOpacity={0.18}
                  stroke="var(--color-requests)"
                  strokeWidth={2}
                  isAnimationActive={false}
                />
                <Area
                  dataKey="errors"
                  type="monotone"
                  fill="var(--color-errors)"
                  fillOpacity={0.14}
                  stroke="var(--color-errors)"
                  strokeWidth={2}
                  isAnimationActive={false}
                />
              </AreaChart>
            </ChartContainer>
          )}
        </CardContent>
      </Card>

      <Card>
        <CardHeader>
          <CardTitle>Token 用量</CardTitle>
        </CardHeader>
        <CardContent>
          {tokenData.length === 0 ? (
            <EmptyState title="当前范围没有 Token 数据" />
          ) : (
            <ChartContainer
              config={tokenConfig}
              className="aspect-auto h-[260px] w-full"
            >
              <BarChart
                data={tokenData}
                margin={{ left: 0, right: 8, top: 8, bottom: 0 }}
              >
                <CartesianGrid vertical={false} />
                <XAxis
                  dataKey="time"
                  tickLine={false}
                  axisLine={false}
                  tickMargin={8}
                  minTickGap={24}
                />
                <YAxis
                  tickLine={false}
                  axisLine={false}
                  width={56}
                  allowDecimals={false}
                  tickFormatter={formatTokens}
                />
                <ChartTooltip
                  cursor={{ fill: 'var(--muted)' }}
                  content={
                    <ChartTooltipContent
                      indicator="dashed"
                      formatter={(value) =>
                        typeof value === 'number'
                          ? formatTokens(value)
                          : String(value)
                      }
                    />
                  }
                />
                <Bar
                  dataKey="tokens"
                  fill="var(--color-tokens)"
                  radius={[4, 4, 0, 0]}
                  isAnimationActive={false}
                />
              </BarChart>
            </ChartContainer>
          )}
        </CardContent>
      </Card>

      <BreakdownPie title="模型占比" empty="当前范围没有模型数据" items={modelData} />
      <BreakdownPie
        title="渠道占比"
        empty="当前范围没有渠道数据"
        items={channelData}
      />
    </div>
  );
}

// Memoized: the dashboard re-renders every second for the live rate cards, but
// the chart inputs only change when a query returns new data.
export default memo(DashboardCharts);
