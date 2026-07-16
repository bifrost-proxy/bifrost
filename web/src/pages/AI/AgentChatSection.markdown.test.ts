import { describe, expect, it } from "vitest";
import {
  chatGptWebCitationSourcesFromNode,
  extractPreviewableMarkdownImages,
  isChatGptWebCitationFavicon,
} from "./AgentChatSection.markdown";

const reutersIcon =
  "https://www.google.com/s2/favicons?domain=https://www.reuters.com&sz=128";
const apIcon = "https://www.google.com/s2/favicons?domain=https://apnews.com&sz=128";

function citationNode(children: unknown[]) {
  return { type: "element", tagName: "a", properties: {}, children };
}

function imageNode(src: string, alt = "") {
  return { type: "element", tagName: "img", properties: { src, alt }, children: [] };
}

describe("Agent Chat GPT Web citation markdown", () => {
  it("recognizes the multi-source favicon and label sequence emitted by GPT Web", () => {
    expect(
      chatGptWebCitationSourcesFromNode(
        citationNode([
          imageNode(reutersIcon),
          { type: "text", value: "Reuters+2" },
          imageNode(apIcon),
          { type: "text", value: "AP News+2" },
        ]),
      ),
    ).toEqual([
      { iconSrc: reutersIcon, label: "Reuters+2" },
      { iconSrc: apIcon, label: "AP News+2" },
    ]);
  });

  it("supports a single GPT Web citation source", () => {
    expect(
      chatGptWebCitationSourcesFromNode(
        citationNode([imageNode(reutersIcon), { type: "text", value: " Reuters " }]),
      ),
    ).toEqual([{ iconSrc: reutersIcon, label: "Reuters" }]);
  });

  it("does not reinterpret ordinary linked images or descriptive image links", () => {
    expect(
      chatGptWebCitationSourcesFromNode(
        citationNode([
          imageNode("https://example.com/chart.png"),
          { type: "text", value: "Quarterly chart" },
        ]),
      ),
    ).toBeNull();
    expect(
      chatGptWebCitationSourcesFromNode(
        citationNode([imageNode(reutersIcon, "Reuters logo"), { type: "text", value: "Reuters" }]),
      ),
    ).toBeNull();
  });

  it("rejects malformed favicon URLs, missing labels, and mixed child nodes", () => {
    expect(isChatGptWebCitationFavicon("javascript:alert(1)")).toBe(false);
    expect(isChatGptWebCitationFavicon("https://example.com/s2/favicons?domain=reuters.com")).toBe(false);
    expect(chatGptWebCitationSourcesFromNode(citationNode([imageNode(reutersIcon)]))).toBeNull();
    expect(
      chatGptWebCitationSourcesFromNode(
        citationNode([
          imageNode(reutersIcon),
          { type: "element", tagName: "strong", properties: {}, children: [] },
        ]),
      ),
    ).toBeNull();
  });

  it("keeps citation favicons out of the lightbox while retaining ordinary images", () => {
    const content = [
      `[![](${reutersIcon})Reuters+2![](${apIcon})AP News+2](https://www.reuters.com/story)`,
      "![Generated chart](https://example.com/chart.png)",
    ].join("\n\n");
    expect(extractPreviewableMarkdownImages(content)).toEqual([
      { alt: "Generated chart", src: "https://example.com/chart.png" },
    ]);
  });

  it("keeps standalone or descriptive favicon images previewable", () => {
    const content = [
      `![](${reutersIcon})`,
      `[![Reuters logo](${reutersIcon})](https://www.reuters.com)`,
    ].join("\n\n");
    expect(extractPreviewableMarkdownImages(content)).toEqual([
      { alt: "", src: reutersIcon },
      { alt: "Reuters logo", src: reutersIcon },
    ]);
  });
});
