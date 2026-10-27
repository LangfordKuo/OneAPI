import { Card, Col, Empty, Row } from 'antd';
import { Column, Line, Pie } from '@ant-design/plots';
import dayjs from 'dayjs';
import type { BreakdownItem, TimeseriesPoint } from '../api/types';

interface Props {
  points: TimeseriesPoint[];
  models: BreakdownItem[];
  channels: BreakdownItem[];
}

function timeLabel(timestamp: number): string {
  return dayjs(timestamp).format('MM-DD HH:mm');
}

export default function DashboardCharts({ points, models, channels }: Props) {
  const requestData = points.flatMap((point) => {
    const time = timeLabel(point.timestamp);
    return [
      { time, value: point.requests, series: '请求' },
      { time, value: point.errors, series: '错误' },
    ];
  });
  const tokenData = points.map((point) => ({
    time: timeLabel(point.timestamp),
    value: point.total_tokens,
  }));
  const modelData = models.map((item) => ({ type: item.label, value: item.requests }));
  const channelData = channels.map((item) => ({ type: item.label, value: item.requests }));

  return (
    <Row gutter={[16, 16]}>
      <Col xs={24} lg={12}>
        <Card title="请求 / 错误时序">
          {requestData.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <Line
              data={requestData}
              xField="time"
              yField="value"
              colorField="series"
              height={260}
              shapeField="smooth"
            />
          )}
        </Card>
      </Col>
      <Col xs={24} lg={12}>
        <Card title="Token 用量">
          {tokenData.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <Column data={tokenData} xField="time" yField="value" height={260} />
          )}
        </Card>
      </Col>
      <Col xs={24} lg={12}>
        <Card title="模型占比">
          {modelData.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <Pie
              data={modelData}
              angleField="value"
              colorField="type"
              height={260}
              innerRadius={0.5}
            />
          )}
        </Card>
      </Col>
      <Col xs={24} lg={12}>
        <Card title="渠道占比">
          {channelData.length === 0 ? (
            <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} />
          ) : (
            <Pie
              data={channelData}
              angleField="value"
              colorField="type"
              height={260}
              innerRadius={0.5}
            />
          )}
        </Card>
      </Col>
    </Row>
  );
}
