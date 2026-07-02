import { useEffect, useRef } from 'react';
import { useSearchParams } from 'react-router-dom';
import { theme } from 'antd';
import SplitPane from '../../components/SplitPane';
import RuleList from './RuleList';
import RuleEditor from './RuleEditor';
import { useRulesStore } from '../../stores/useRulesStore';
import { useValuesStore } from '../../stores/useValuesStore';
import { notifyApiBusinessError } from '../../api/client';
import pushService from '../../services/pushService';

const GROUP_PARAM = 'group';
const RULE_PARAM = 'rule';

function getUrlParams(): URLSearchParams {
  const hash = window.location.hash;
  const qIdx = hash.indexOf('?');
  const raw = qIdx >= 0 ? hash.slice(qIdx) : window.location.search;
  return new URLSearchParams(raw);
}

function syncGroupFromUrl() {
  const store = useRulesStore.getState();
  const params = getUrlParams();
  const groupParam = params.get(GROUP_PARAM);
  const initialGroupId = groupParam || null;
  if (initialGroupId !== store.activeGroupId) {
    useRulesStore.setState({
      activeGroupId: initialGroupId,
      isGroupMode: initialGroupId !== null,
      groupWritable: false,
      rules: [],
      selectedRuleName: null,
      currentRule: null,
      editingContent: {},
    });
  }
}

syncGroupFromUrl();

export default function Rules() {
  const { token } = theme.useToken();
  const [searchParams, setSearchParams] = useSearchParams();
  const targetGroupParam = searchParams.get(GROUP_PARAM);
  const targetRuleParam = searchParams.get(RULE_PARAM);
  const error = useRulesStore((state) => state.error);
  const clearError = useRulesStore((state) => state.clearError);
  const rules = useRulesStore((state) => state.rules);
  const selectedRuleName = useRulesStore((state) => state.selectedRuleName);
  const selectRule = useRulesStore((state) => state.selectRule);
  const fetchRules = useRulesStore((state) => state.fetchRules);
  const setActiveGroupId = useRulesStore((state) => state.setActiveGroupId);
  const activeGroupId = useRulesStore((state) => state.activeGroupId);
  const loading = useRulesStore((state) => state.loading);
  const applyValuesSnapshot = useValuesStore((state) => state.applyValuesSnapshot);

  const initDoneRef = useRef(false);
  const hasLoadedOnceRef = useRef(false);
  const restoringRef = useRef(false);

  useEffect(() => {
    const storeHasState = rules.length > 0 && selectedRuleName !== null;

    if (storeHasState) {
      initDoneRef.current = true;
      hasLoadedOnceRef.current = true;
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
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (loading) {
      hasLoadedOnceRef.current = true;
    }
  }, [loading]);

  useEffect(() => {
    if (initDoneRef.current) return;
    if (loading) return;
    if (!hasLoadedOnceRef.current) return;

    if (rules.length === 0) return;
    initDoneRef.current = true;
    restoringRef.current = true;

    const ruleParam = targetRuleParam;
    if (ruleParam) {
      const exists = rules.some((r) => r.name === ruleParam);
      if (exists) {
        selectRule(ruleParam);
        return;
      }
    }
    selectRule(rules[0].name);
  }, [rules, loading, targetRuleParam, selectRule]);

  useEffect(() => {
    if (!initDoneRef.current) return;

    const targetGroupId = targetGroupParam || null;
    if (targetGroupId !== activeGroupId) {
      setActiveGroupId(targetGroupId);
      void fetchRules().then(() => {
        if (targetRuleParam) {
          void selectRule(targetRuleParam);
        }
      });
      return;
    }

    if (targetRuleParam && targetRuleParam !== selectedRuleName) {
      restoringRef.current = true;
      void selectRule(targetRuleParam);
    }
  }, [
    activeGroupId,
    fetchRules,
    selectRule,
    selectedRuleName,
    setActiveGroupId,
    targetGroupParam,
    targetRuleParam,
  ]);

  useEffect(() => {
    if (restoringRef.current) {
      restoringRef.current = false;
      return;
    }
    if (!initDoneRef.current) return;
    if (rules.length > 0 && !selectedRuleName) {
      selectRule(rules[0].name);
    }
  }, [rules, selectedRuleName, selectRule]);

  useEffect(() => {
    if (!initDoneRef.current) return;
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (activeGroupId) {
        next.set(GROUP_PARAM, activeGroupId);
      } else {
        next.delete(GROUP_PARAM);
      }
      if (selectedRuleName) {
        next.set(RULE_PARAM, selectedRuleName);
      } else {
        next.delete(RULE_PARAM);
      }
      if (next.toString() === prev.toString()) return prev;
      return next;
    }, { replace: true });
  }, [activeGroupId, selectedRuleName, setSearchParams]);

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
    display: 'flex' as const,
    flexDirection: 'column' as const,
    backgroundColor: token.colorBgContainer,
    position: 'relative' as const,
  };

  return (
    <div style={containerStyle}>
      <div style={{ flex: 1, overflow: 'hidden' }}>
        <SplitPane
          left={<RuleList />}
          right={<RuleEditor />}
          defaultLeftWidth="250px"
          minLeftWidth={200}
          minRightWidth={400}
        />
      </div>
    </div>
  );
}
