import { type ClipboardEvent, type CSSProperties } from "react";
import { Button } from "antd";
import { DeleteOutlined } from "@ant-design/icons";
import type { PendingChatImage } from "./AgentChatSection.helpers";

export const MAX_PASTED_IMAGES = 6;

export function imageCountLabel(count: number) {
  return count === 1 ? "Attached 1 image" : `Attached ${count} images`;
}

export function imageContentParts(content: string, images: PendingChatImage[]) {
  if (images.length === 0) {
    return undefined;
  }
  return [
    ...(content.trim() ? [{ type: "text" as const, text: content }] : []),
    ...images.map((image) => ({
      type: "image_url" as const,
      image_url: { url: image.previewUrl, detail: "auto" },
    })),
  ];
}

export function imageSizeLabel(size: number) {
  if (size >= 1024 * 1024) {
    return `${(size / (1024 * 1024)).toFixed(1)} MB`;
  }
  if (size >= 1024) {
    return `${Math.ceil(size / 1024)} KB`;
  }
  return `${size} B`;
}

export function imageFilesFromClipboard(event: ClipboardEvent<HTMLTextAreaElement>) {
  const files = Array.from(event.clipboardData.files);
  const itemFiles = Array.from(event.clipboardData.items)
    .filter((item) => item.kind === "file")
    .map((item) => item.getAsFile())
    .filter((file): file is File => Boolean(file));
  return [...files, ...itemFiles].filter((file, index, allFiles) =>
    file.type.startsWith("image/") &&
    allFiles.findIndex(
      (candidate) =>
        candidate.name === file.name &&
        candidate.size === file.size &&
        candidate.type === file.type,
    ) === index,
  );
}

export function pendingImageFromFile(file: File): Promise<PendingChatImage | undefined> {
  return new Promise((resolve) => {
    const reader = new FileReader();
    reader.onload = () => {
      const previewUrl = String(reader.result || "");
      if (!previewUrl.startsWith("data:")) {
        resolve(undefined);
        return;
      }
      const data = previewUrl.split(",", 2)[1] || "";
      resolve({
        id: `image-${Date.now()}-${Math.random().toString(36).slice(2)}`,
        mimeType: file.type || "image/png",
        data,
        previewUrl,
        name: file.name || undefined,
        size: file.size,
      });
    };
    reader.onerror = () => resolve(undefined);
    reader.readAsDataURL(file);
  });
}

export type AgentChatImagePreviewStyles = Record<string, CSSProperties>;

export function AgentChatImagePreviewStrip({
  images,
  onRemove,
  styles,
}: {
  images: PendingChatImage[];
  onRemove: (imageId: string) => void;
  styles: AgentChatImagePreviewStyles;
}) {
  if (images.length === 0) {
    return null;
  }
  return (
    <div style={styles.imagePreviewStrip} data-testid="agent-chat-image-preview-strip">
      {images.map((image, index) => (
        <div key={image.id} style={styles.imagePreviewItem} data-testid="agent-chat-image-preview">
          <img
            src={image.previewUrl}
            alt={image.name || `Pasted image ${index + 1}`}
            style={styles.imagePreviewThumb}
          />
          <Button
            size="small"
            type="text"
            icon={<DeleteOutlined />}
            aria-label={`Remove pasted image ${index + 1}`}
            style={styles.imagePreviewRemove}
            onClick={() => onRemove(image.id)}
          />
          <span style={styles.imagePreviewMeta}>{imageSizeLabel(image.size)}</span>
        </div>
      ))}
    </div>
  );
}
