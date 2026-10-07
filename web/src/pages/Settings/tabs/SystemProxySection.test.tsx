import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { ConfigProvider, theme } from "antd";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SystemProxyStatus } from "../../../api/proxy";
import SystemProxySection from "./SystemProxySection";

const suspended: SystemProxyStatus = {
  supported: true,
  enabled: false,
  managed_by_bifrost: false,
  configured_enabled: true,
  host: "",
  port: 0,
  bypass: "",
};

describe.each(["light", "dark"])("SystemProxySection (%s)", (appearance) => {
  let container: HTMLDivElement;
  let root: Root;
  const onToggleSystemProxy = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal("IS_REACT_ACT_ENVIRONMENT", true);
    vi.stubGlobal("matchMedia", (query: string) => ({
      matches: false,
      media: query,
      addListener: vi.fn(),
      removeListener: vi.fn(),
    }));
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  async function render(systemProxy: SystemProxyStatus) {
    await act(async () => {
      root.render(
        <ConfigProvider
          theme={{
            algorithm:
              appearance === "dark"
                ? theme.darkAlgorithm
                : theme.defaultAlgorithm,
          }}
        >
          <SystemProxySection
            systemProxy={systemProxy}
            systemProxyLaunchd={null}
            cliProxy={null}
            systemProxyLoading={false}
            systemProxyLaunchdLoading={false}
            injectBifrostBadge={false}
            injectBifrostBadgeLoading={false}
            onToggleSystemProxy={onToggleSystemProxy}
            onToggleSystemProxyLaunchd={vi.fn()}
            onToggleInjectBifrostBadge={vi.fn()}
          />
        </ConfigProvider>,
      );
    });
    return container.querySelector<HTMLButtonElement>(
      '[data-testid="settings-system-proxy-switch"]',
    )!;
  }

  it("keeps suspended intent checked and lets the user turn it off", async () => {
    const toggle = await render(suspended);

    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toContain("configured but not active");
    expect(container.textContent).not.toContain(
      "Route all system traffic through this proxy",
    );

    await act(async () => toggle.click());

    expect(onToggleSystemProxy).toHaveBeenCalledOnce();
    expect(onToggleSystemProxy.mock.calls[0][0]).toBe(false);

    await render({ ...suspended, configured_enabled: false });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(container.textContent).not.toContain("configured but not active");
  });

  it("warns when intent is off but the OS proxy is still active", async () => {
    const toggle = await render({
      ...suspended,
      configured_enabled: false,
      enabled: true,
      managed_by_bifrost: true,
    });

    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(container.textContent).toContain("System proxy is still active");
    expect(container.textContent).toContain(
      "saved system proxy preference is off",
    );
    expect(container.textContent).toContain(
      "OS is still using Bifrost as its system proxy",
    );
    expect(container.textContent).not.toContain(
      "Route all system traffic through this proxy",
    );
    expect(container.textContent).not.toContain("configured but not active");

    await render({ ...suspended, configured_enabled: false });
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(container.textContent).not.toContain("System proxy is still active");
  });

  it.each([true, false])(
    "keeps external ownership truthful when configured intent is %s",
    async (configured_enabled) => {
      const toggle = await render({
        ...suspended,
        enabled: true,
        configured_enabled,
        host: "127.0.0.1",
        port: 6152,
      });

      expect(toggle.getAttribute("aria-checked")).toBe(
        String(configured_enabled),
      );
      expect(container.textContent).toContain(
        "System proxy is occupied by another proxy",
      );
      expect(container.textContent).toContain("127.0.0.1:6152");
      if (configured_enabled) {
        expect(container.textContent).toContain("not currently active");
        expect(container.textContent).not.toContain("Turn this on");
      } else {
        expect(container.textContent).toContain("Turn this on");
      }
    },
  );
});
