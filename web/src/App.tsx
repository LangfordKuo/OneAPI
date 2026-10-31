import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { AlertCircle } from 'lucide-react';
import { lazy, Suspense, useEffect, type ReactNode } from 'react';
import { Navigate, Route, Routes, useNavigate } from 'react-router-dom';
import { api, UNAUTHORIZED_EVENT } from './api/client';
import type { Admin, SetupStatus, SiteInfo } from './api/types';
import { PageLoader } from './components/common/page-loader';
import { AppShell } from './components/layout/app-shell';
import { Alert, AlertDescription, AlertTitle } from './components/ui/alert';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from './components/ui/card';
import { LiveMetricsProvider } from './hooks/useLiveMetrics';
import { errorMessage } from './lib/error';
import { toast } from 'sonner';
import Login from './pages/Login';
import Setup from './pages/Setup';

const Dashboard = lazy(() => import('./pages/Dashboard'));
const Channels = lazy(() => import('./pages/Channels'));
const Keys = lazy(() => import('./pages/Keys'));
const Logs = lazy(() => import('./pages/Logs'));
const Playground = lazy(() => import('./pages/Playground'));
const Account = lazy(() => import('./pages/Account'));
const SettingsPage = lazy(() => import('./pages/Settings'));

function CenteredShell({ children }: { children: ReactNode }) {
  return (
    <div className="flex min-h-screen items-center justify-center p-6">
      {children}
    </div>
  );
}

export default function App() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  // Both `request`/`requestRaw` and the SSE stream report an expired session
  // through this event. Invalidating `['me']` makes the app drop back to the
  // login screen, which unmounts the authenticated shell and its SSE provider.
  useEffect(() => {
    const onUnauthorized = () => {
      void queryClient.invalidateQueries({ queryKey: ['me'] });
      navigate('/', { replace: true });
    };
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => {
      window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    };
  }, [navigate, queryClient]);

  const siteQuery = useQuery({
    queryKey: ['site'],
    queryFn: () => api.get<SiteInfo>('/api/site'),
    retry: false,
  });
  const siteName = siteQuery.data?.site_name ?? 'OneAPI';
  const setupQuery = useQuery({
    queryKey: ['setup-status'],
    queryFn: () => api.get<SetupStatus>('/api/setup/status'),
    retry: false,
  });
  const meQuery = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Admin>('/api/auth/me'),
    retry: false,
    enabled: setupQuery.data?.initialized === true,
  });

  const logout = useMutation({
    mutationFn: () => api.post('/api/auth/logout'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['me'] });
    },
    // Without this a failed logout leaves the user signed in with no feedback.
    onError: (error) => {
      toast.error(errorMessage(error));
    },
  });

  if (setupQuery.isLoading) {
    return <PageLoader label="正在连接 OneAPI…" />;
  }

  if (setupQuery.isError) {
    return (
      <CenteredShell>
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>无法连接后端</CardTitle>
            <CardDescription>请确认 OneAPI 服务正在运行。</CardDescription>
          </CardHeader>
          <CardContent>
            <Alert variant="destructive">
              <AlertCircle />
              <AlertTitle>连接失败</AlertTitle>
              <AlertDescription>
                {setupQuery.error instanceof Error
                  ? setupQuery.error.message
                  : String(setupQuery.error)}
              </AlertDescription>
            </Alert>
          </CardContent>
        </Card>
      </CenteredShell>
    );
  }

  const setup = setupQuery.data;
  if (!setup) {
    return <PageLoader label="正在读取初始化状态…" />;
  }

  if (!setup.initialized) {
    return (
      <Setup
        status={setup}
        siteName={siteName}
        onSuccess={() => window.location.assign('/')}
      />
    );
  }

  if (meQuery.isLoading) {
    return <PageLoader label="正在验证会话…" />;
  }

  if (meQuery.isError || !meQuery.data) {
    return (
      <Login
        siteName={siteName}
        version={siteQuery.data?.version}
        onSuccess={() => void meQuery.refetch()}
      />
    );
  }

  return (
    <LiveMetricsProvider>
      <AppShell
        siteName={siteName}
        version={siteQuery.data?.version}
        username={meQuery.data.username}
        onLogout={() => logout.mutate()}
      >
        <Suspense fallback={<PageLoader />}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/channels" element={<Channels />} />
            <Route path="/keys" element={<Keys />} />
            <Route path="/logs" element={<Logs />} />
            <Route path="/playground" element={<Playground />} />
            <Route path="/account" element={<Account />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </Suspense>
      </AppShell>
    </LiveMetricsProvider>
  );
}
