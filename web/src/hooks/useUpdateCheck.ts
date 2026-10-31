import { useQuery, type QueryClient } from '@tanstack/react-query';
import { api } from '@/api/client';
import type { UpdateInfo } from '@/api/types';

const CACHE_KEY = 'oneapi.update-check.v1';
const DAY_MS = 24 * 60 * 60 * 1000;

/** Shared cache key so the shell badge and the settings page stay in sync. */
export const UPDATE_CHECK_QUERY_KEY = ['update-check'] as const;

interface CachedCheck {
  checked_at: number;
  info: UpdateInfo;
}

function readCache(): CachedCheck | null {
  try {
    const raw = window.localStorage.getItem(CACHE_KEY);
    if (!raw) {
      return null;
    }
    const parsed = JSON.parse(raw) as CachedCheck;
    if (
      typeof parsed?.checked_at !== 'number' ||
      parsed.info == null ||
      typeof parsed.info.current_version !== 'string'
    ) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

function writeCache(info: UpdateInfo): void {
  try {
    window.localStorage.setItem(
      CACHE_KEY,
      JSON.stringify({ checked_at: Date.now(), info }),
    );
  } catch {
    // Private mode or a full quota must not break the update flow.
  }
}

/**
 * Fetch the latest release. Unless `force` is set, a result younger than a day
 * is reused from localStorage, so the program checks at most once per day and a
 * page reload does not hit the update source again.
 */
export async function fetchUpdateInfo(force = false): Promise<UpdateInfo> {
  if (!force) {
    const cached = readCache();
    if (cached && Date.now() - cached.checked_at < DAY_MS) {
      return cached.info;
    }
  }
  const info = await api.get<UpdateInfo>('/api/update/check');
  writeCache(info);
  return info;
}

/**
 * Daily update check backing the version badge. Failures are silent here: the
 * badge simply stays clean, and the settings page reports errors when the
 * administrator checks manually.
 */
export function useUpdateCheck() {
  return useQuery({
    queryKey: UPDATE_CHECK_QUERY_KEY,
    queryFn: () => fetchUpdateInfo(),
    staleTime: DAY_MS,
    refetchInterval: DAY_MS,
    retry: 0,
  });
}

/**
 * Manual check that bypasses the day cache and refreshes the shared query, so
 * the version badge reflects the result immediately.
 */
export async function refreshUpdateCheck(
  queryClient: QueryClient,
): Promise<UpdateInfo> {
  const info = await fetchUpdateInfo(true);
  queryClient.setQueryData(UPDATE_CHECK_QUERY_KEY, info);
  return info;
}
