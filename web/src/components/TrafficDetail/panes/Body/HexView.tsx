import { useMemo, useRef, useState, useEffect } from 'react';
import { theme, Button } from 'antd';
import type { SessionTargetSearchState } from '../../../../types';
import { useTextSelection } from '../../hooks/useTextSelection';
import { useMarkSearch } from '../../hooks/useMarkSearch';
import { DEFAULT_SHOW_MAX_SIZE } from '../../helper/contentType';
import { bytesFromBase64, toHexView, toHexViewFromBytes } from './hex';

interface HexViewProps {
  data?: string | null;
  dataBase64?: string | null;
  searchValue: SessionTargetSearchState;
  onSearch: (v: Partial<SessionTargetSearchState>) => void;
}

export const HexView = ({ data, dataBase64, searchValue, onSearch }: HexViewProps) => {
  const { token } = theme.useToken();
  const [showAll, setShowAll] = useState(false);
  const wrapperRef = useTextSelection(!!data);
  const contentRef = useRef<HTMLPreElement>(null);

  const truncatedData = useMemo(() => {
    if (!data) return '';
    if (!showAll && data.length > DEFAULT_SHOW_MAX_SIZE) {
      return data.substring(0, DEFAULT_SHOW_MAX_SIZE);
    }
    return data;
  }, [data, showAll]);

  const decodedBytes = useMemo(() => {
    if (!dataBase64) return null;
    try {
      return bytesFromBase64(dataBase64);
    } catch {
      return null;
    }
  }, [dataBase64]);

  const truncatedBytes = useMemo(() => {
    if (!decodedBytes) return null;
    if (!showAll && decodedBytes.length > DEFAULT_SHOW_MAX_SIZE) {
      return decodedBytes.slice(0, DEFAULT_SHOW_MAX_SIZE);
    }
    return decodedBytes;
  }, [decodedBytes, showAll]);

  const hexData = useMemo(() => {
    if (truncatedBytes) return toHexViewFromBytes(truncatedBytes);
    if (!truncatedData) return '';
    return toHexView(truncatedData);
  }, [truncatedBytes, truncatedData]);

  const totalSize = decodedBytes?.length ?? data?.length ?? 0;
  const shouldShowMore = !showAll && totalSize > DEFAULT_SHOW_MAX_SIZE;

  const { startMarkSearch } = useMarkSearch(
    searchValue,
    () => contentRef.current,
    onSearch
  );

  useEffect(() => {
    const el = contentRef.current;
    if (!el) return;
    el.textContent = hexData;
  }, [hexData]);

  useEffect(() => {
    if (!searchValue.value) return;
    startMarkSearch();
  }, [hexData, searchValue.value, startMarkSearch]);

  if (!data && !dataBase64) {
    return null;
  }

  return (
    <div ref={wrapperRef} style={{ position: 'relative' }}>
      <pre
        ref={contentRef}
        style={{
          margin: 0,
          padding: 8,
          fontSize: 11,
          fontFamily: 'monospace',
          backgroundColor: token.colorBgLayout,
          borderRadius: 4,
          whiteSpace: 'pre',
          overflowX: 'auto',
          lineHeight: 1.4,
        }}
      />
      {shouldShowMore && (
        <Button
          type="link"
          onClick={() => setShowAll(true)}
          style={{
            position: 'absolute',
            bottom: 8,
            right: 8,
            background: token.colorBgContainer,
          }}
        >
          Show All ({Math.round((totalSize - DEFAULT_SHOW_MAX_SIZE) / 1024)}KB more)
        </Button>
      )}
    </div>
  );
};
