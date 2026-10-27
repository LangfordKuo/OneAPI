import { Card, Empty } from 'antd';

interface PlaceholderProps {
  title: string;
  milestone: string;
}

export default function Placeholder({ title, milestone }: PlaceholderProps) {
  return (
    <Card title={title}>
      <Empty description={`${title}将在 ${milestone} 实现`} />
    </Card>
  );
}
