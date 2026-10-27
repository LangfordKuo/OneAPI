import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Button, Layout, Menu, Space, Spin, Typography } from 'antd';
import { Link, Navigate, Route, Routes, useLocation } from 'react-router-dom';
import { api } from './api/client';
import type { Admin } from './api/types';
import Channels from './pages/Channels';
import Dashboard from './pages/Dashboard';
import Keys from './pages/Keys';
import Login from './pages/Login';
import Placeholder from './pages/Placeholder';
import SettingsPage from './pages/Settings';

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
  const meQuery = useQuery({
    queryKey: ['me'],
    queryFn: () => api.get<Admin>('/api/auth/me'),
    retry: false,
  });

  const logout = useMutation({
    mutationFn: () => api.post('/api/auth/logout'),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ['me'] });
    },
  });

  if (meQuery.isLoading) {
    return (
      <div className="center-shell">
        <Spin size="large" />
      </div>
    );
  }

  if (meQuery.isError) {
    return <Login onSuccess={() => void meQuery.refetch()} />;
  }

  return (
    <Layout style={{ minHeight: '100vh' }}>
      <Sider theme="light" width={220}>
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
            background: '#fff',
            display: 'flex',
            justifyContent: 'space-between',
            paddingInline: 24,
          }}
        >
          <Typography.Text type="secondary">统一 AI API 网关</Typography.Text>
          <Space>
            <Typography.Text>{meQuery.data?.username}</Typography.Text>
            <Button size="small" onClick={() => logout.mutate()} loading={logout.isPending}>
              退出
            </Button>
          </Space>
        </Header>
        <Content style={{ padding: 24 }}>
          <Routes>
            <Route path="/" element={<Dashboard />} />
            <Route path="/channels" element={<Channels />} />
            <Route path="/keys" element={<Keys />} />
            <Route path="/logs" element={<Placeholder title="请求日志" milestone="M6" />} />
            <Route path="/playground" element={<Placeholder title="Playground" milestone="M6" />} />
            <Route path="/settings" element={<SettingsPage />} />
            <Route path="*" element={<Navigate to="/" replace />} />
          </Routes>
        </Content>
      </Layout>
    </Layout>
  );
}
