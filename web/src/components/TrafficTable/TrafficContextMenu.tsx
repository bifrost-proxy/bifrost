import { useCallback, useEffect, useRef, useMemo, useLayoutEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { Menu, message, Modal } from 'antd';
import type { MenuProps } from 'antd';
import {
  CopyOutlined,
  CodeOutlined,
  DownloadOutlined,
  UnlockOutlined,
  SendOutlined,
  ExportOutlined,
  AppstoreOutlined,
} from '@ant-design/icons';
import type { TrafficRecord, TrafficSummary } from '../../types';
import { generateCurl } from '../../utils/curl';
import { downloadHAR } from '../../utils/har';
import { copyToClipboard } from '../../utils/clipboard';
import { getTrafficDetail, getRequestBody, getResponseBody } from '../../api/traffic';
import { isNotFoundError } from '../../api/client';
import { useReplayStore } from '../../stores/useReplayStore';
import { useExportBifrost } from '../../hooks/useExportBifrost';
import { useTlsConfigStore } from '../../stores/useTlsConfigStore';
import { showTlsWhitelistChangeSuccess } from '../../utils/tlsInterceptionNotice';
import { useI18n } from '../../i18n';

const MENU_WIDTH = 220;

interface TrafficContextMenuProps {
  record: TrafficSummary | null;
  visible: boolean;
  position: { x: number; y: number };
  onClose: () => void;
  selectedRecords?: TrafficSummary[];
}

export default function TrafficContextMenu({
  record,
  visible,
  position,
  onClose,
  selectedRecords = [],
}: TrafficContextMenuProps) {
  const { t } = useI18n();
  const menuRef = useRef<HTMLDivElement>(null);
  const navigate = useNavigate();
  const importFromTraffic = useReplayStore((state) => state.importFromTraffic);
  const { exportFile } = useExportBifrost();
  const {
    fetchConfig: fetchTlsConfig,
    addDomainToIntercept,
    addAppToIntercept,
    addIpToIntercept,
    isDomainInIntercept,
    isAppInIntercept,
    isIpInIntercept,
    config: tlsConfig,
  } = useTlsConfigStore();

  const isTunnel = record?.is_tunnel || record?.method === 'CONNECT';
  const hasHost = !!record?.host;
  const hasClientApp = !!record?.client_app;
  const hasClientIp = !!record?.client_ip;

  const hasMultipleSelected = selectedRecords.length > 1;

  useEffect(() => {
    if (visible && !tlsConfig) {
      fetchTlsConfig();
    }
  }, [visible, tlsConfig, fetchTlsConfig]);

  const domainAlreadyInIntercept = record?.host ? isDomainInIntercept(record.host) : false;
  const appAlreadyInIntercept = record?.client_app ? isAppInIntercept(record.client_app) : false;
  const ipAlreadyInIntercept = record?.client_ip ? isIpInIntercept(record.client_ip) : false;

  useLayoutEffect(() => {
    const menuEl = menuRef.current;
    if (!visible || !menuEl) return;

    const padding = 8;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const menuWidth = menuEl.offsetWidth || MENU_WIDTH;
    const menuHeight = menuEl.offsetHeight || 0;

    let newX = position.x;
    let newY = position.y;

    if (newX + menuWidth > viewportWidth - padding) {
      newX = Math.max(padding, position.x - menuWidth);
    }

    if (newY + menuHeight > viewportHeight - padding) {
      newY = Math.max(padding, viewportHeight - menuHeight - padding);
    }

    menuEl.style.left = `${newX}px`;
    menuEl.style.top = `${newY}px`;
    menuEl.style.visibility = 'visible';

    return () => {
      menuEl.style.visibility = 'hidden';
    };
  }, [visible, position]);

  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        onClose();
      }
    };

    const handleEscape = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        onClose();
      }
    };

    if (visible) {
      document.addEventListener('mousedown', handleClickOutside);
      document.addEventListener('keydown', handleEscape);
    }

    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
      document.removeEventListener('keydown', handleEscape);
    };
  }, [visible, onClose]);

  const copyUrl = useCallback(async () => {
    if (!record) return;
    try {
      await copyToClipboard(record.url);
      message.success(t('traffic.contextMenu.urlCopied'));
    } catch {
      message.error(t('traffic.contextMenu.copyUrlFailed'));
    }
    onClose();
  }, [record, onClose, t]);

  const copyAsCurl = useCallback(async () => {
    if (!record) return;
    try {
      let fullRecord: TrafficRecord;
      try {
        const [detail, requestBody] = await Promise.all([
          getTrafficDetail(record.id),
          getRequestBody(record.id).catch(() => null),
        ]);
        fullRecord = { ...detail, request_body: requestBody };
      } catch (fetchError) {
        if (isNotFoundError(fetchError)) {
          fullRecord = {
            ...record,
            request_headers: null,
            response_headers: null,
            request_body: null,
            response_body: null,
            matched_rules: null,
            request_content_type: null,
          };
        } else {
          throw fetchError;
        }
      }
      const curl = generateCurl(fullRecord);
      await copyToClipboard(curl);
      message.success(t('traffic.contextMenu.curlCopied'));
    } catch (error) {
      message.error(t('traffic.contextMenu.curlFailed'));
      console.error(error);
    }
    onClose();
  }, [record, onClose, t]);

  const downloadAsHAR = useCallback(async () => {
    const recordsToExport = selectedRecords.length > 1 ? selectedRecords : (record ? [record] : []);
    if (recordsToExport.length === 0) return;

    try {
      message.loading({ content: t('traffic.contextMenu.generatingHar'), key: 'har-download' });
      const fullRecords: TrafficRecord[] = [];
      
      for (const r of recordsToExport) {
        const detail = await getTrafficDetail(r.id);
        const [requestBody, responseBody] = await Promise.all([
          getRequestBody(r.id),
          getResponseBody(r.id),
        ]);
        fullRecords.push({
          ...detail,
          request_body: requestBody,
          response_body: responseBody,
        });
      }
      
      downloadHAR(fullRecords);
      message.success({ content: t('traffic.contextMenu.harDownloaded', { count: fullRecords.length }), key: 'har-download' });
    } catch (error) {
      message.error({ content: t('traffic.contextMenu.harDownloadFailed'), key: 'har-download' });
      console.error(error);
    }
    onClose();
  }, [record, selectedRecords, onClose, t]);

  const addDomainToInterceptList = useCallback(() => {
    if (!record?.host) return;

    const host = record.host;
    onClose();

    Modal.confirm({
      title: t('traffic.contextMenu.addDomainTitle'),
      content: t('traffic.contextMenu.addDomainContent', { host }),
      okText: t('common.add'),
      cancelText: t('common.cancel'),
      onOk: async () => {
        try {
          const success = await addDomainToIntercept(host);
          if (success) {
            showTlsWhitelistChangeSuccess(t('traffic.contextMenu.addDomainSuccess', { host }));
          } else {
            message.error(t('traffic.contextMenu.addDomainFailed'));
          }
        } catch (error) {
          message.error(t('traffic.contextMenu.addDomainFailed'));
          console.error(error);
        }
      },
    });
  }, [record, onClose, addDomainToIntercept, t]);

  const addAppToInterceptList = useCallback(() => {
    if (!record?.client_app) return;

    const app = record.client_app;
    onClose();

    Modal.confirm({
      title: t('traffic.contextMenu.addAppTitle'),
      content: t('traffic.contextMenu.addAppContent', { app }),
      okText: t('common.add'),
      cancelText: t('common.cancel'),
      onOk: async () => {
        try {
          const success = await addAppToIntercept(app);
          if (success) {
            showTlsWhitelistChangeSuccess(t('traffic.contextMenu.addAppSuccess', { app }));
          } else {
            message.error(t('traffic.contextMenu.addAppFailed'));
          }
        } catch (error) {
          message.error(t('traffic.contextMenu.addAppFailed'));
          console.error(error);
        }
      },
    });
  }, [record, onClose, addAppToIntercept, t]);

  const addIpToInterceptList = useCallback(() => {
    if (!record?.client_ip) return;

    const ip = record.client_ip;
    onClose();

    Modal.confirm({
      title: t('traffic.contextMenu.addIpTitle'),
      content: t('traffic.contextMenu.addIpContent', { ip }),
      okText: t('common.add'),
      cancelText: t('common.cancel'),
      onOk: async () => {
        try {
          const success = await addIpToIntercept(ip);
          if (success) {
            showTlsWhitelistChangeSuccess(t('traffic.contextMenu.addIpSuccess', { ip }));
          } else {
            message.error(t('traffic.contextMenu.addIpFailed'));
          }
        } catch (error) {
          message.error(t('traffic.contextMenu.addIpFailed'));
          console.error(error);
        }
      },
    });
  }, [record, onClose, addIpToIntercept, t]);

  const replayRequest = useCallback(async () => {
    if (!record) return;
    onClose();
    await importFromTraffic(record.id);
    navigate('/replay');
  }, [record, onClose, importFromTraffic, navigate]);

  const exportAsBifrost = useCallback(async () => {
    const recordsToExport = selectedRecords.length > 1 ? selectedRecords : (record ? [record] : []);
    if (recordsToExport.length === 0) return;

    const recordIds = recordsToExport.map((r) => r.id);
    await exportFile('network', { record_ids: recordIds, include_body: true });
    onClose();
  }, [record, selectedRecords, exportFile, onClose]);

  const interceptMenuItems: MenuProps['items'] = useMemo(() => {
    const items: NonNullable<MenuProps['items']> = [];
    if (hasHost && !domainAlreadyInIntercept) {
      items.push({
        key: 'add-domain-intercept',
        icon: <UnlockOutlined />,
        label: t('traffic.contextMenu.interceptTarget', { target: record?.host }),
        onClick: addDomainToInterceptList,
      });
    }
    if (hasClientApp && !appAlreadyInIntercept) {
      items.push({
        key: 'add-app-intercept',
        icon: <AppstoreOutlined />,
        label: t('traffic.contextMenu.interceptTarget', { target: record?.client_app }),
        onClick: addAppToInterceptList,
      });
    }
    if (hasClientIp && !ipAlreadyInIntercept) {
      items.push({
        key: 'add-ip-intercept',
        icon: <UnlockOutlined />,
        label: t('traffic.contextMenu.interceptIp', { ip: record?.client_ip }),
        onClick: addIpToInterceptList,
      });
    }
    return items;
  }, [hasHost, hasClientApp, hasClientIp, domainAlreadyInIntercept, appAlreadyInIntercept, ipAlreadyInIntercept, record, addDomainToInterceptList, addAppToInterceptList, addIpToInterceptList, t]);

  if (!visible || !record) return null;

  const menuItems: MenuProps['items'] = hasMultipleSelected
    ? [
        {
          key: 'download-har',
          icon: <DownloadOutlined />,
          label: t('traffic.contextMenu.downloadManyAsHar', { count: selectedRecords.length }),
          onClick: downloadAsHAR,
        },
        {
          key: 'export-bifrost',
          icon: <ExportOutlined />,
          label: t('traffic.contextMenu.exportManyAsBifrost', { count: selectedRecords.length }),
          onClick: exportAsBifrost,
        },
      ]
    : [
        ...(!isTunnel ? [
          {
            key: 'replay',
            icon: <SendOutlined />,
            label: t('traffic.contextMenu.replay'),
            onClick: replayRequest,
          },
          { type: 'divider' as const },
        ] : []),
        {
          key: 'copy-url',
          icon: <CopyOutlined />,
          label: t('traffic.contextMenu.copyUrl'),
          onClick: copyUrl,
        },
        {
          key: 'copy-curl',
          icon: <CodeOutlined />,
          label: t('traffic.contextMenu.copyAsCurl'),
          onClick: copyAsCurl,
        },
        {
          key: 'download-har',
          icon: <DownloadOutlined />,
          label: t('traffic.contextMenu.downloadAsHar'),
          onClick: downloadAsHAR,
        },
        {
          key: 'export-bifrost',
          icon: <ExportOutlined />,
          label: t('traffic.contextMenu.exportAsBifrost'),
          onClick: exportAsBifrost,
        },
        ...(interceptMenuItems.length > 0 ? [
          { type: 'divider' as const },
          ...interceptMenuItems,
        ] : []),
      ];

  return (
    <div
      ref={menuRef}
      style={{
        position: 'fixed',
        left: position.x,
        top: position.y,
        zIndex: 1050,
        visibility: 'hidden',
        boxShadow: '0 6px 16px 0 rgba(0, 0, 0, 0.08), 0 3px 6px -4px rgba(0, 0, 0, 0.12), 0 9px 28px 8px rgba(0, 0, 0, 0.05)',
        borderRadius: 8,
      }}
    >
      <Menu
        items={menuItems}
        style={{ borderRadius: 8 }}
        mode="vertical"
      />
    </div>
  );
}
