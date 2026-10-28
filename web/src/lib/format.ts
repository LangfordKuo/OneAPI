import dayjs from 'dayjs';

export function formatDateTime(
  value: number | null | undefined,
  pattern = 'YYYY-MM-DD HH:mm:ss',
): string {
  return value == null ? '-' : dayjs(value).format(pattern);
}

export function formatNumber(
  value: number | null | undefined,
  fallback = '-',
): string {
  return value == null ? fallback : value.toLocaleString('zh-CN');
}

export function formatLatency(value: number | null | undefined): string {
  return value == null ? '-' : `${value} ms`;
}

export function formatTokensPerSecond(
  value: number | null | undefined,
): string {
  return value == null ? '-' : `${value.toFixed(1)} tok/s`;
}

export function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (days > 0) {
    return `${days} 天 ${hours} 小时`;
  }
  if (hours > 0) {
    return `${hours} 小时 ${minutes} 分`;
  }
  return `${minutes} 分`;
}
