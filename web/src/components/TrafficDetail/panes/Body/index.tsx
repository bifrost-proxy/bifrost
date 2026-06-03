import { Segmented, Typography, theme } from 'antd';
import type { CSSProperties } from 'react';
import type {
  RecordContentType,
  SessionTargetSearchState,
  DisplayFormat,
} from '../../../../types';
import { DisplayFormat as DF } from '../../../../types';
import { HighLightBody } from './HighLightBody';
import { HexView } from './HexView';
import { TreeView } from './TreeView';
import { shouldDisableJsonStructuredView } from '../../helper/contentType';

const { Text } = Typography;

const styles: Record<string, CSSProperties> = {
  mediaContainer: {
    padding: 12,
    backgroundColor: 'transparent',
    borderRadius: 4,
    textAlign: 'center',
    height: '100%',
    overflow: 'auto',
  },
  mediaImage: {
    maxWidth: '100%',
    maxHeight: '100%',
    objectFit: 'contain',
    borderRadius: 4,
  },
};

interface BodyProps {
  data?: string | null;
  rawData?: string | null;
  rawDataBase64?: string | null;
  source?: 'decoded' | 'raw';
  onSourceChange?: (source: 'decoded' | 'raw') => void;
  contentType: RecordContentType;
  rawContentType?: string | null;
  mediaSrc?: string | null;
  searchValue: SessionTargetSearchState;
  displayFormat: DisplayFormat;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
  editable?: boolean;
  onBodyChange?: (value: string) => void;
}

export const Body = ({
  data,
  rawData,
  rawDataBase64,
  source = 'decoded',
  onSourceChange,
  contentType,
  rawContentType,
  mediaSrc,
  searchValue,
  displayFormat,
  onSearch,
  editable = false,
  onBodyChange,
}: BodyProps) => {
  const { token } = theme.useToken();
  const normalizedRawContentType = rawContentType?.toLowerCase() ?? '';
  const isImageMedia = contentType === 'Media' && normalizedRawContentType.includes('image/');
  const hasRawData = Boolean(rawData || rawDataBase64);
  const displayData = source === 'raw' && hasRawData ? (rawData ?? '') : data;
  const displayDataBase64 = source === 'raw' ? rawDataBase64 : null;
  const disableJsonStructuredView = shouldDisableJsonStructuredView(contentType, displayData);
  const sourceSwitch = hasRawData && onSourceChange ? (
    <div style={{ padding: '6px 8px 0 8px' }}>
      <Segmented
        size="small"
        value={source}
        onChange={(value) => onSourceChange(value as 'decoded' | 'raw')}
        options={[
          { label: 'Decoded', value: 'decoded' },
          { label: 'Raw', value: 'raw' },
        ]}
      />
    </div>
  ) : null;

  if (displayFormat === DF.Media) {
    if (contentType === 'Media') {
      if (isImageMedia && mediaSrc) {
        return (
          <div
            style={{
              ...styles.mediaContainer,
              backgroundColor: token.colorBgLayout,
            }}
          >
            <img
              src={mediaSrc}
              alt={rawContentType || 'Response preview'}
              style={styles.mediaImage}
            />
          </div>
        );
      }
      return (
        <div
          style={{
            ...styles.mediaContainer,
            backgroundColor: token.colorBgLayout,
          }}
        >
          <Text type="secondary">Media preview not supported</Text>
        </div>
      );
    }
    return (
      <Text type="secondary" style={{ padding: 12, display: 'block' }}>
        Not a media type
      </Text>
    );
  }

  if (!displayData && !displayDataBase64) {
    return (
      <>
        {sourceSwitch}
        <Text type="secondary" style={{ padding: 8, display: 'block' }}>
          No body content
        </Text>
      </>
    );
  }

  if (displayFormat === DF.Hex) {
    return (
      <>
        {sourceSwitch}
        <HexView
          data={displayData}
          dataBase64={displayDataBase64}
          searchValue={searchValue}
          onSearch={onSearch}
        />
      </>
    );
  }

  if (displayFormat === DF.Tree && !disableJsonStructuredView) {
    return (
      <>
        {sourceSwitch}
        <TreeView data={displayData} searchValue={searchValue} onSearch={onSearch} />
      </>
    );
  }

  return (
    <>
      {sourceSwitch}
      <HighLightBody
        data={displayData}
        contentType={contentType}
        searchValue={searchValue}
        onSearch={onSearch}
        editable={editable}
        onBodyChange={onBodyChange}
      />
    </>
  );
};

export { BodyTypeMenu } from './BodyTypeMenu';
