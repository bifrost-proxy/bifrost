import { useMemo } from 'react';
import { Dropdown, Typography, Space } from 'antd';
import { DownOutlined, CheckOutlined } from '@ant-design/icons';
import type { MenuProps } from 'antd';
import { DisplayFormat } from '../../../../types';
import type { RecordContentType } from '../../../../types';
import { shouldDisableJsonStructuredView } from '../../helper/contentType';
import { useI18n } from '../../../../i18n';

interface BodyTypeMenuProps {
  value: string;
  onChange: (v: string) => void;
  contentType: RecordContentType;
  data?: string | null;
}

export const BodyTypeMenu = ({
  value,
  onChange,
  contentType,
  data,
}: BodyTypeMenuProps) => {
  const { t } = useI18n();
  const disableJsonStructuredView = shouldDisableJsonStructuredView(contentType, data);

  const menuItems = useMemo<MenuProps['items']>(() => {
    const items: MenuProps['items'] = [];

    if (contentType === 'Media') {
      items.push({
        key: DisplayFormat.Media,
        label: (
          <Space>
            {value === DisplayFormat.Media && <CheckOutlined />}
            {t('trafficDetail.bodyType.media')}
          </Space>
        ),
      });
    }

    if (contentType !== 'Media') {
      items.push({
        key: DisplayFormat.HighLight,
        label: (
          <Space>
            {value === DisplayFormat.HighLight && <CheckOutlined />}
            {contentType === 'JSON'
              ? disableJsonStructuredView
                ? t('trafficDetail.bodyType.text')
                : 'JSON'
              : contentType === 'Other'
                ? t('trafficDetail.bodyType.other')
                : contentType}
          </Space>
        ),
      });
    }

    if (contentType === 'JSON' && !disableJsonStructuredView) {
      items.push({
        key: DisplayFormat.Tree,
        label: (
          <Space>
            {value === DisplayFormat.Tree && <CheckOutlined />}
            {t('trafficDetail.bodyType.tree')}
          </Space>
        ),
      });
    }

    items.push({
      key: DisplayFormat.Hex,
      label: (
        <Space>
          {value === DisplayFormat.Hex && <CheckOutlined />}
          Hex
        </Space>
      ),
    });

    return items;
  }, [contentType, disableJsonStructuredView, value, t]);

  const activeLabel = useMemo(() => {
    switch (value) {
      case DisplayFormat.HighLight:
        return contentType === 'JSON'
          ? disableJsonStructuredView
            ? t('trafficDetail.bodyType.text')
            : 'JSON'
          : contentType === 'Other'
            ? t('trafficDetail.bodyType.other')
            : contentType;
      case DisplayFormat.Hex:
        return 'Hex';
      case DisplayFormat.Tree:
        return disableJsonStructuredView
          ? t('trafficDetail.bodyType.text')
          : t('trafficDetail.bodyType.tree');
      case DisplayFormat.Media:
        return t('trafficDetail.bodyType.media');
      default:
        return t('trafficDetail.bodyType.view');
    }
  }, [value, contentType, disableJsonStructuredView, t]);

  const handleClick: MenuProps['onClick'] = ({ key }) => {
    onChange(key);
  };

  return (
    <Dropdown menu={{ items: menuItems, onClick: handleClick }} trigger={['click']}>
      <Typography.Text
        type="secondary"
        style={{ cursor: 'pointer', fontSize: 12 }}
      >
        {activeLabel} <DownOutlined style={{ fontSize: 10 }} />
      </Typography.Text>
    </Dropdown>
  );
};
