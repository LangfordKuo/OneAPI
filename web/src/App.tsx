import { lazy, Suspense, useEffect, useState, type ReactNode } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import {
  Button,
  ConfigProvider,
  Layout,
  Menu,
  Space,
  Spin,
  Switch,
  theme,
  Typography,
} from 'antd';
import { Link, Navigate, Route, Routes, useLocation } from 'react-router-dom';
import { api } from './api/client';
import type { Admin } from './api/types';
import Login from './pages/Login';

const Dashboard = lazy(() => import('./pages/Dashboard'));
const Channels = lazy(() => import('./pages/Channels'));
const Keys = lazy(() => import('./pages/Keys'));
const Logs = lazy(() => import('./pages/Logs'));
const Playground = lazy(() => import('./pages/Playground'));
const SettingsPage = lazy(() => import('./pages/Settings'));

const { Content, Header, Sider } = Layout;

const items = [
  { key: '/', label: <Link to="/">仪表盘</Link> },
  { key: '/channels', label: <Link to="/channels">渠道</Link> },
  { key: '/keys', label: <Link to="/keys">API Keys</Link> },
  { key: '/logs', label: <Link to="/logs">日志</Link> },
  { key: '/playground', label: <Link to="/playground">Playground</Link> },
  { key: '/settings', label: <Link to="/settings">设置</Link> },
];

function selectedKey(pathname: string) {
  if (pathname === '/') {
    return '/';
  }

  return (
    items.find((item) => item.key !== '/' && pathname.startsWith(item.key))?.key ?? '/'
  );
}

export default function App() {
  const location = useLocation();
  const queryClient = useQueryClient();
  const [dark, setDark] = useState(
    () => window.localStorage.getItem('oneapi-theme') === 'dark',
  );
  const meQuery = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Admin>('/api/auth/me'),
    retry: false,
  });

  useEffect(() => {
    window.localStorage.setItem('oneapi-theme', dark ? 'dark' : 'light');
    document.body.style.background = dark ? '#000' : '';
  }, [dark]);

  const logout = useMutation({
    mutationFn: () => api.post('/api/auth/logout'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['me'] });
    },
  });

  const themed = (content: ReactNode) => (
    <ConfigProvider
      theme={{ algorithm: dark ? theme.darkAlgorithm : theme.defaultAlgorithm }}
    >
      {content}
    </ConfigProvider>
  );

  if (meQuery.isLoading) {
    return themed(
      <div className="center-shell">
        <Spin size="large" />
      </div>,
    );
  }

  if (meQuery.isError) {
    return themed(<Login onSuccess={() => void meQuery.refetch()} />);
  }

  return themed(
    <Layout style={{ minHeight: '100vh' }}>
      <Sider theme={dark ? 'dark' : 'light'} width={220}>
        <div style={{ padding: '20px 16px', fontSize: 18, fontWeight: 600 }}>OneAPI</div>
        <Menu
          mode="inline"
          selectedKeys={[selectedKey(location.pathname)]}
          items={items}
          style={{ borderInlineEnd: 0 }}
        />
      </Sider>
      <Layout>
        <Header
          style={{
            alignItems: 'center',
            background: dark ? '#141414' : '#fff',
            display: 'flex',
            justifyContent: 'space-between',
            paddingInline: 24,
          }}
        >
          <Typography.Text type="secondary">统一 AI API 网关</Typography.Text>
          <Space>
            <Switch
              checked={dark}
              checkedChildren="暗色"
              unCheckedChildren="亮色"
              onChange={setDark}
            />
            <Typography.Text>{meQuery.data?.username}</Typography.Text>
            <Button size="small" onClick={() => logout.mutate()} loading={logout.isPending}>
              退出
            </Button>
          </Space>
        </Header>
        <Content style={{ padding: 24 }}>
          <Suspense
            fallback={
              <div className="center-shell">
                <Spin size="large" />
              </div>
            }
          >
            <Routes>
              <Route path="/" element={<Dashboard />} />
              <Route path="/channels" element={<Channels />} />
              <Route path="/keys" element={<Keys />} />
              <Route path="/logs" element={<Logs />} />
              <Route path="/playground" element={<Playground />} />
              <Route path="/settings" element={<SettingsPage />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </Suspense>
        </Content>
      </Layout>
    </Layout>,
  );
}
