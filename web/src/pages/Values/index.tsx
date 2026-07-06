import { useEffect, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import { theme } from 'antd';
import SplitPane from '../../components/SplitPane';
import ValueList from './ValueList';
import ValueEditor from './ValueEditor';
import { useValuesStore } from '../../stores/useValuesStore';
import { notifyApiBusinessError } from '../../api/client';
import pushService from '../../services/pushService';
import { isMacDesktopShell } from '../../runtime';

export default function Values() {
  const { token } = theme.useToken();
  const [searchParams, setSearchParams] = useSearchParams();
  const error = useValuesStore((state) => state.error);
  const clearError = useValuesStore((state) => state.clearError);
  const values = useValuesStore((state) => state.values);
  const selectedValueName = useValuesStore((state) => state.selectedValueName);
  const selectValue = useValuesStore((state) => state.selectValue);
  const fetchValues = useValuesStore((state) => state.fetchValues);
  const applyValuesSnapshot = useValuesStore((state) => state.applyValuesSnapshot);

  const initRef = useRef(false);
  const urlParamRef = useRef(false);
  const loadedRef = useRef(false);

  useEffect(() => {
    if (loadedRef.current) return;
    loadedRef.current = true;

    if (values.length === 0) {
      void fetchValues();
    }
    pushService.connect({ need_values: true });
    const unsubscribe = pushService.onValuesUpdate((data) => {
      applyValuesSnapshot(data.values);
    });

    return () => {
      unsubscribe();
      pushService.updateSubscription({ need_values: false });
      pushService.disconnectIfIdle();
    };
  }, [applyValuesSnapshot, fetchValues, values.length]);

  useEffect(() => {
    const nameParam = searchParams.get('name');
    if (nameParam && values.length > 0 && !urlParamRef.current) {
      urlParamRef.current = true;
      const exists = values.some(v => v.name === nameParam);
      if (exists) {
        selectValue(nameParam);
        setSearchParams({}, { replace: true });
      }
    }
  }, [searchParams, values, selectValue, setSearchParams]);

  useEffect(() => {
    if (initRef.current || urlParamRef.current) return;
    if (values.length > 0 && !selectedValueName) {
      initRef.current = true;
      selectValue(values[0].name);
    }
  }, [values, selectedValueName, selectValue]);

  useEffect(() => {
    if (error) {
      notifyApiBusinessError(new Error(error), error);
      clearError();
    }
  }, [error, clearError]);

  const containerStyle = {
    width: '100%',
    height: '100%',
    overflow: 'hidden',
    backgroundColor: isMacDesktopShell() ? 'transparent' : token.colorBgContainer,
  };

  return (
    <div style={containerStyle}>
      <SplitPane
        left={<ValueList />}
        right={<ValueEditor />}
        defaultLeftWidth="250px"
        minLeftWidth={200}
        minRightWidth={400}
      />
    </div>
  );
}
