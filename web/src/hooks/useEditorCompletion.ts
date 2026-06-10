import { useEffect } from 'react';
import { useValuesStore } from '../stores/useValuesStore';
import { useScriptsStore } from '../stores/useScriptsStore';
import { updateDynamicData } from '../components/BifrostEditor';

let unsubValues: (() => void) | null = null;
let unsubScripts: (() => void) | null = null;

function setupSubscriptions() {
  if (unsubValues) unsubValues();
  if (unsubScripts) unsubScripts();

  unsubValues = useValuesStore.subscribe((state) => {
    updateDynamicData({
      values: state.values.map((v) => v.name),
    });
  });

  unsubScripts = useScriptsStore.subscribe((state) => {
    updateDynamicData({
      requestScripts: state.requestScripts.map((s) => s.name),
      responseScripts: state.responseScripts.map((s) => s.name),
    });
  });
}

setupSubscriptions();

export function useEditorCompletion() {
  useEffect(() => {
    useScriptsStore.getState().fetchScripts();
    syncDynamicData();
  }, []);
}

export function syncDynamicData() {
  const valuesState = useValuesStore.getState();
  updateDynamicData({
    values: valuesState.values.map((v) => v.name),
  });

  const scriptsState = useScriptsStore.getState();
  updateDynamicData({
    requestScripts: scriptsState.requestScripts.map((s) => s.name),
    responseScripts: scriptsState.responseScripts.map((s) => s.name),
  });
}

export function resetEditorCompletion() {
  updateDynamicData({
    values: [],
    rules: [],
    requestScripts: [],
    responseScripts: [],
  });
}
