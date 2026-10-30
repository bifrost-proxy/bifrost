import "../../../src/index.css";
import { useState } from "react";
import { createRoot } from "react-dom/client";
import SearchMode from "../../../src/components/SearchMode";
import { useSearchStore } from "../../../src/stores/useSearchStore";
import { useTrafficStore } from "../../../src/stores/useTrafficStore";
import { useFilterPanelStore } from "../../../src/stores/useFilterPanelStore";
import { useLocaleStore } from "../../../src/i18n/store";
useLocaleStore.getState().setLocale("en");
useSearchStore.getState().reset();
useSearchStore.getState().setMode("search");
Object.assign(window, {
  searchStore: useSearchStore,
  trafficStore: useTrafficStore,
  filterStore: useFilterPanelStore,
});
function Harness() {
  const [selectedId, setSelectedId] = useState<string>();
  return (
    <div style={{ height: 600 }}>
      <SearchMode
        selectedId={selectedId}
        onSelect={(r) => setSelectedId(r.id)}
        onDoubleClick={() => {}}
      />
      <output data-testid="detail">{selectedId}</output>
    </div>
  );
}
createRoot(document.getElementById("root")!).render(<Harness />);

declare global {
  interface Window {
    searchStore: typeof useSearchStore;
    trafficStore: typeof useTrafficStore;
    filterStore: typeof useFilterPanelStore;
    firstRow: Element;
    scrollContainer: HTMLElement;
  }
}
