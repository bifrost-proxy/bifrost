import { describe, expect, it } from "vitest";
import {
  imageContentParts,
  imageCountLabel,
  imageSizeLabel,
} from "./AgentChatSection.images";
import type { PendingChatImage } from "./AgentChatSection.helpers";

function pendingImage(overrides: Partial<PendingChatImage> = {}): PendingChatImage {
  return {
    id: "image-1",
    mimeType: "image/png",
    data: "aGVsbG8=",
    previewUrl: "data:image/png;base64,aGVsbG8=",
    name: "pasted.png",
    size: 2048,
    ...overrides,
  };
}

describe("Agent Chat pasted image helpers", () => {
  it("labels pure image messages the same way as the composer", () => {
    expect(imageCountLabel(1)).toBe("Attached 1 image");
    expect(imageCountLabel(3)).toBe("Attached 3 images");
  });

  it("builds multimodal content parts with text before pasted images", () => {
    expect(imageContentParts("Describe this", [pendingImage()])).toEqual([
      { type: "text", text: "Describe this" },
      {
        type: "image_url",
        image_url: { url: "data:image/png;base64,aGVsbG8=", detail: "auto" },
      },
    ]);
  });

  it("builds image-only content parts without empty text", () => {
    expect(imageContentParts("   ", [pendingImage({ previewUrl: "data:image/jpeg;base64,aW1n" })])).toEqual([
      {
        type: "image_url",
        image_url: { url: "data:image/jpeg;base64,aW1n", detail: "auto" },
      },
    ]);
  });

  it("formats preview sizes compactly", () => {
    expect(imageSizeLabel(512)).toBe("512 B");
    expect(imageSizeLabel(2048)).toBe("2 KB");
    expect(imageSizeLabel(1536 * 1024)).toBe("1.5 MB");
  });
});
