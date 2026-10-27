import { Button, Form, Input, Space } from 'antd';

interface KeyValueListProps {
  name: string;
  keyPlaceholder: string;
  valuePlaceholder: string;
}

export default function KeyValueList({
  name,
  keyPlaceholder,
  valuePlaceholder,
}: KeyValueListProps) {
  return (
    <Form.List name={name}>
      {(fields, { add, remove }) => (
        <>
          {fields.map(({ key, name: fieldName, ...restField }) => (
            <Space key={key} align="baseline" style={{ display: 'flex', marginBottom: 8 }}>
              <Form.Item
                {...restField}
                name={[fieldName, 'key']}
                rules={[{ required: true, message: '请输入键' }]}
                style={{ marginBottom: 0 }}
              >
                <Input placeholder={keyPlaceholder} />
              </Form.Item>
              <Form.Item
                {...restField}
                name={[fieldName, 'value']}
                rules={[{ required: true, message: '请输入值' }]}
                style={{ marginBottom: 0 }}
              >
                <Input placeholder={valuePlaceholder} />
              </Form.Item>
              <Button type="link" danger onClick={() => remove(fieldName)}>
                删除
              </Button>
            </Space>
          ))}
          <Button type="dashed" block onClick={() => add()}>
            添加
          </Button>
        </>
      )}
    </Form.List>
  );
}
