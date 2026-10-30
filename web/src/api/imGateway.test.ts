import { describe, expect, it } from "vitest";
import { selectableRunnerIds, type ExternalCliGatewayConfig } from "./imGateway";

describe("Runner selection after browser Runner retirement", () => {
  it("keeps supported Runners and leaves retired records available for migration", () => {
    const config = {
      defaultRunnerId: "web",
      runners: Object.fromEntries(
        ["codex", "traex", "claude_code", "custom_cli", "chatgpt_web", "gpt-web"].map(
          (adapter) => [adapter, { enabled: true, adapter, adapterConfig: {} }],
        ),
      ),
      channels: {},
    } as ExternalCliGatewayConfig;
    expect(selectableRunnerIds(config)).toEqual(["claude_code", "codex", "custom_cli", "traex"]);
    expect(config.defaultRunnerId).toBe("web");
    expect(config.runners.chatgpt_web.adapter).toBe("chatgpt_web");
  });
});
