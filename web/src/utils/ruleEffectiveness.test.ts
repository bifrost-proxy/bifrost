import { describe, expect, test } from "vitest";
import { analyzeRuleEffectiveness } from "./ruleEffectiveness";

const nextOncallSample = `# Global default rules.
# These rules are always enabled and apply to every proxy listener.

# * proxy://10.71.185.109:6152

# abc

# * dns://10.71.128.1
https://app.example.com/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_billing_skip_app_tcc","x-use-ppe":"1"}
https://app.example.com/api/v1/oncall/ passthrough://
https://nextoncall-bd.byteintl.net/api/v1/oncall/feature_extract/evaluation/ reqHeaders://{"x-tt-env":"ppe_feature_wxeb9i","x-use-ppe":"1"}
https://nextoncall-bd.byteintl.net/api/v1/oncall/feature_extract/evaluation/ passthrough://
# === nextoncall_local ===

https://app.example.com/api/v1/oncall/ reqHeaders://{"x-tt-env":"ppe_ticket_system","x-use-ppe":"1"}
https://app.example.com/api/v1/oncall/ passthrough://
https://app.example.com/api/v1/summary/ reqHeaders://{"x-tt-env":"ppe_ticket_system","x-use-ppe":"1"}
https://app.example.com/api/v1/summary/ passthrough://`;

describe("analyzeRuleEffectiveness", () => {
  test("marks comments and blank lines as neutral", () => {
    const effects = analyzeRuleEffectiveness("# comment\n\nexample.test statusCode://204");

    expect(effects[0]).toMatchObject({ status: "neutral", summary: "Comment line" });
    expect(effects[1]).toMatchObject({ status: "neutral", summary: "Blank line" });
    expect(effects[2]).toMatchObject({ status: "active" });
  });

  test("marks later same matcher passthrough as covered by the first selected passthrough", () => {
    const effects = analyzeRuleEffectiveness(nextOncallSample);

    const firstPassthrough = effects.find(
      (effect) =>
        effect.text === "https://app.example.com/api/v1/oncall/ passthrough://",
    );
    const laterPassthrough = effects.find(
      (effect) =>
        effect.lineNumber > (firstPassthrough?.lineNumber ?? 0) &&
        effect.text === "https://app.example.com/api/v1/oncall/ passthrough://",
    );

    expect(firstPassthrough).toMatchObject({ status: "active" });
    expect(laterPassthrough).toMatchObject({
      status: "shadowed",
      coveredByLine: firstPassthrough?.lineNumber,
    });
    expect(laterPassthrough?.summary).toContain("Covered by line");
  });

  test("marks earlier same matcher request headers as covered by later equal-priority duplicates", () => {
    const effects = analyzeRuleEffectiveness(nextOncallSample);

    const firstHeaders = effects.find((effect) =>
      effect.text.includes("ppe_billing_skip_app_tcc"),
    );
    const laterHeaders = effects.find((effect) =>
      effect.text.includes("ppe_ticket_system"),
    );

    expect(firstHeaders).toMatchObject({
      status: "shadowed",
      coveredByLine: laterHeaders?.lineNumber,
    });
    expect(firstHeaders?.details.join("\n")).toContain("x-tt-env");
    expect(laterHeaders).toMatchObject({ status: "active" });
  });

  test("treats ampersand-separated request headers as independent override fields", () => {
    const effects = analyzeRuleEffectiveness(
      [
        "https://example.test/api/ reqHeaders://(x-env=one&x-stable=keep)",
        "https://example.test/api/ reqHeaders://x-env=two",
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "partial", coveredByLine: 2 });
    expect(effects[0].details.join("\n")).toContain("x-env");
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("keeps different protocols on the same matcher active when they do not compete", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `https://example.test/api/ reqHeaders://{"x-env":"one"}`,
        `https://example.test/api/ passthrough://`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("marks a request header line as partial when only some headers are already written", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `https://example.test/api/ reqHeaders://{"x-env":"one","x-stable":"keep"}`,
        `https://example.test/api/ reqHeaders://{"x-env":"two"}`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "partial", coveredByLine: 2 });
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("keeps ampersands inside parenthesized JSON header values", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `https://example.test/api/ reqHeaders://({"x-query":"a=1&stable=yes"})`,
        `https://example.test/api/ reqHeaders://stable=overridden`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("keeps delimiters inside header template expressions", () => {
    const effects = analyzeRuleEffectiveness(
      [
        "https://example.test/api/ reqHeaders://(x-host=${hostname.replace(no&x-fake=1,replaced)}&x-mode=active)",
        "https://example.test/api/ reqHeaders://x-fake=overridden",
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[0].details.join("\n")).not.toContain("x-fake");
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("marks broader path request headers as partial when a narrower matcher wins one header", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `https://example.test/api/internal/ reqHeaders://{"x-env":"narrow"}`,
        `https://example.test/api/ reqHeaders://{"x-env":"broad","x-stable":"keep"}`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[1]).toMatchObject({ status: "partial", coveredByLine: 1 });
    expect(effects[1].summary).toContain("partially covered");
    expect(effects[1].details.join("\n")).toContain("narrower part");
    expect(effects[1].details.join("\n")).toContain("outside that narrower scope");
  });

  test("detects wildcard host partial coverage for concrete host request headers", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `https://api.example.test/private/ reqHeaders://{"x-env":"api-private"}`,
        `https://*.example.test/private/ reqHeaders://{"x-env":"all-private"}`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[1]).toMatchObject({ status: "partial", coveredByLine: 1 });
    expect(effects[1].details.join("\n")).toContain("overlapping matcher traffic");
  });

  test("marks global request headers as partial when a concrete matcher wins a subset", () => {
    const effects = analyzeRuleEffectiveness(
      [
        `* reqHeaders://{"x-env":"global"}`,
        `https://example.test/api/ reqHeaders://{"x-env":"api"}`,
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "partial", coveredByLine: 2 });
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("marks same matcher single-match statusCode duplicate as covered by the higher priority parse winner", () => {
    const effects = analyzeRuleEffectiveness(
      [
        "https://example.test/api/ statusCode://201",
        "https://example.test/api/ statusCode://202",
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "active" });
    expect(effects[1]).toMatchObject({ status: "shadowed", coveredByLine: 1 });
  });

  test("marks equal-priority urlParams with the same key as later-wins", () => {
    const effects = analyzeRuleEffectiveness(
      [
        "https://example.test/api/ urlParams://(trace:first,stable:one)",
        "https://example.test/api/ urlParams://(trace:second)",
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "partial", coveredByLine: 2 });
    expect(effects[1]).toMatchObject({ status: "active" });
  });

  test("marks equal-priority last-value body operations as later-wins", () => {
    const effects = analyzeRuleEffectiveness(
      [
        "https://example.test/api/ resBody://(first)",
        "https://example.test/api/ resBody://(second)",
      ].join("\n"),
    );

    expect(effects[0]).toMatchObject({ status: "shadowed", coveredByLine: 2 });
    expect(effects[1]).toMatchObject({ status: "active" });
  });
});
