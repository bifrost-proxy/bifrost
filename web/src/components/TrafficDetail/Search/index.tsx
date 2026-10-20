import { useEffect, useRef } from 'react';
import { Input, Space, Typography } from 'antd';
import { CloseOutlined, UpOutlined, DownOutlined } from '@ant-design/icons';
import type { InputRef } from 'antd';
import type { SessionTargetSearchState } from '../../../types';
import { useI18n } from '../../../i18n';

interface SearchProps {
  value: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
}

export const Search = ({ value: searchValue, onSearch }: SearchProps) => {
  const { t: tr } = useI18n();
  const inputRef = useRef<InputRef>(null);
  const { value, show, total, next = 1 } = searchValue;

  const onChange = (v?: string) => {
    onSearch({ value: v, next: 1 });
  };

  const onClear = () => {
    onChange(undefined);
  };

  const onNext = () => {
    let t = next + 1;
    if (total && t > total) {
      t = 1;
    }
    onSearch({ next: t });
  };

  const onPrev = () => {
    let t = next - 1;
    if (t < 1) {
      t = total || 1;
    }
    onSearch({ next: t });
  };

  const onPressEnter = () => {
    onNext();
  };

  useEffect(() => {
    if (show) {
      inputRef.current?.focus();
    } else {
      onSearch({ value: undefined, next: 0 });
    }
  }, [show, onSearch]);

  useEffect(() => {
    if (next) {
      inputRef.current?.blur();
      setTimeout(() => inputRef.current?.focus(), 0);
    }
  }, [next]);

  if (!show) {
    return null;
  }

  return (
    <div style={{ padding: '8px 0', display: 'flex', alignItems: 'center', gap: 8 }}>
      <Input
        ref={inputRef}
        size="small"
        placeholder={tr("trafficDetail.search.placeholder")}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        onPressEnter={onPressEnter}
        style={{ width: 200 }}
        suffix={
          <Space size={4}>
            {value && (
              <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                {total ? `${next}/${total}` : '0'}
              </Typography.Text>
            )}
            <UpOutlined
              onClick={onPrev}
              style={{ fontSize: 10, cursor: 'pointer', color: '#999' }}
            />
            <DownOutlined
              onClick={onNext}
              style={{ fontSize: 10, cursor: 'pointer', color: '#999' }}
            />
            <CloseOutlined
              onClick={onClear}
              style={{ fontSize: 10, cursor: 'pointer', color: '#999' }}
            />
          </Space>
        }
      />
    </div>
  );
};
