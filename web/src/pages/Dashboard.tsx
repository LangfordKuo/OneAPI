import { useQuery } from '@tanstack/react-query';
import { Alert, Card, Col, Descriptions, Row, Statistic, Tag } from 'antd';
import { api } from '../api/client';

interface SystemInfo {
  name: string;
  version: string;
  uptime_seconds: number;
  database: string;
  data_dir: string;
}

function formatUptime(seconds: number): string {
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

export default function Dashboard() {
  const { data, error, isLoading } = useQuery({
    queryKey: ['system'],
    queryFn: () => api.get<SystemInfo>('/api/system'),
    refetchInterval: 30_000,
  });

  if (error) {
    return (
      <Alert
        type="error"
        showIcon
        message="无法连接后端"
        description={error instanceof Error ? error.message : String(error)}
      />
    );
  }

  return (
    <>
      <Row gutter={[16, 16]}>
        <Col xs={24} md={8}>
          <Card loading={isLoading}>
            <Statistic title="服务状态" value={data?.database === 'ok' ? '正常' : '未知'} />
          </Card>
        </Col>
        <Col xs={24} md={8}>
          <Card loading={isLoading}>
            <Statistic
              title="运行时长"
              value={data ? formatUptime(data.uptime_seconds) : '-'}
            />
          </Card>
        </Col>
        <Col xs={24} md={8}>
          <Card loading={isLoading}>
            <Statistic title="版本" value={data?.version ?? '-'} />
          </Card>
        </Col>
      </Row>
      <Card title="系统信息" style={{ marginTop: 16 }} loading={isLoading}>
        <Descriptions column={1} size="small">
          <Descriptions.Item label="名称">{data?.name}</Descriptions.Item>
          <Descriptions.Item label="数据库">
            <Tag color={data?.database === 'ok' ? 'green' : 'red'}>{data?.database}</Tag>
          </Descriptions.Item>
          <Descriptions.Item label="数据目录">{data?.data_dir}</Descriptions.Item>
        </Descriptions>
      </Card>
    </>
  );
}
