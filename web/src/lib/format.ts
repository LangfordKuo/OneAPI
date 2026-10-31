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

/** Compact axis/tooltip label: 12345 -> 1.2万, 120000000 -> 1.2亿. */
export function formatCompactNumber(
  value: number | null | undefined,
): string {
  if (value == null) {
    return '-';
  }
  const abs = Math.abs(value);
  if (abs >= 100_000_000) {
    return `${trimTrailingZero(value / 100_000_000)}亿`;
  }
  if (abs >= 10_000) {
    return `${trimTrailingZero(value / 10_000)}万`;
  }
  return value.toLocaleString('zh-CN');
}

function trimTrailingZero(value: number): string {
  return value.toFixed(1).replace(/\.0$/, '');
}

/**
 * Compact token count for narrow layouts: 1234 -> 1.2K, 1234567 -> 1.2M,
 * 1234567890 -> 1.2B. Counts below 1000 stay exact.
 */
export function formatTokens(value: number | null | undefined): string {
  if (value == null) {
    return '-';
  }
  const abs = Math.abs(value);
  if (abs < 1_000) {
    return String(Math.trunc(value));
  }
  const [divisor, suffix] =
    abs >= 1_000_000_000
      ? [1_000_000_000, 'B']
      : abs >= 1_000_000
        ? [1_000_000, 'M']
        : [1_000, 'K'];
  return `${trimTrailingZero(value / divisor)}${suffix}`;
}

export function formatSeconds(value: number | null | undefined): string {
  if (value == null) {
    return '-';
  }
  const seconds = value / 1000;
  return `${seconds.toFixed(seconds >= 10 ? 1 : 2)} s`;
}

export function formatBytes(value: number | null | undefined): string {
  if (value == null) {
    return '-';
  }
  if (value < 1024) {
    return `${value} B`;
  }
  const units = ['KB', 'MB', 'GB', 'TB'];
  let size = value / 1024;
  let unit = 0;
  while (size >= 1024 && unit < units.length - 1) {
    size /= 1024;
    unit += 1;
  }
  return `${size.toFixed(size >= 10 ? 0 : 1)} ${units[unit]}`;
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
