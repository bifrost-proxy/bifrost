import { useState, useRef, useCallback, useEffect } from "react";
import type { ReactNode, CSSProperties } from "react";
import { theme } from "antd";

interface VerticalSplitPaneProps {
  top: ReactNode;
  bottom: ReactNode;
  defaultTopHeight?: string;
  minTopHeight?: number;
  minBottomHeight?: number;
  topHeight?: string;
  onTopHeightChange?: (height: string) => void;
}

export default function VerticalSplitPane({
  top,
  bottom,
  defaultTopHeight = "60%",
  minTopHeight = 150,
  minBottomHeight = 100,
  topHeight: controlledTopHeight,
  onTopHeightChange,
}: VerticalSplitPaneProps) {
  const { token } = theme.useToken();
  const paneBackground = token.colorBgContainer;
  const containerRef = useRef<HTMLDivElement>(null);
  const [internalTopHeight, setInternalTopHeight] = useState<string>(defaultTopHeight);
  const [isDragging, setIsDragging] = useState(false);
  const [isHovering, setIsHovering] = useState(false);

  const topHeight = controlledTopHeight ?? internalTopHeight;

  const styles: Record<string, CSSProperties> = {
    container: {
      display: "flex",
      flexDirection: "column",
      width: "100%",
      height: "100%",
      overflow: "hidden",
      backgroundColor: paneBackground,
    },
    topPane: {
      width: "100%",
      overflow: "hidden",
      flexShrink: 0,
      minHeight: 0,
      display: "flex",
      flexDirection: "column",
      backgroundColor: paneBackground,
    },
    bottomPane: {
      flex: 1,
      width: "100%",
      overflow: "hidden",
      minHeight: 0,
      display: "flex",
      flexDirection: "column",
      backgroundColor: paneBackground,
    },
    divider: {
      height: 4,
      cursor: "row-resize",
      backgroundColor: token.colorBorderSecondary,
      flexShrink: 0,
      transition: "background-color 0.2s",
    },
    dividerHover: {
      backgroundColor: token.colorPrimary,
    },
  };

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  const handleMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!isDragging || !containerRef.current) return;

      const containerRect = containerRef.current.getBoundingClientRect();
      const containerHeight = containerRect.height;
      const newTopHeight = e.clientY - containerRect.top;

      const maxTopHeight = containerHeight - minBottomHeight - 4;

      if (newTopHeight >= minTopHeight && newTopHeight <= maxTopHeight) {
        const newHeight = `${newTopHeight}px`;
        setInternalTopHeight(newHeight);
        onTopHeightChange?.(newHeight);
      }
    },
    [isDragging, minTopHeight, minBottomHeight, onTopHeightChange]
  );

  const handleMouseUp = useCallback(() => {
    setIsDragging(false);
  }, []);

  useEffect(() => {
    if (isDragging) {
      document.addEventListener("mousemove", handleMouseMove);
      document.addEventListener("mouseup", handleMouseUp);
      document.body.style.cursor = "row-resize";
      document.body.style.userSelect = "none";
    }

    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, [isDragging, handleMouseMove, handleMouseUp]);

  const dividerStyle: CSSProperties = {
    ...styles.divider,
    ...(isHovering || isDragging ? styles.dividerHover : {}),
  };

  return (
    <div ref={containerRef} style={styles.container}>
      <div style={{ ...styles.topPane, height: topHeight }}>{top}</div>
      <div
        style={dividerStyle}
        onMouseDown={handleMouseDown}
        onMouseEnter={() => setIsHovering(true)}
        onMouseLeave={() => setIsHovering(false)}
      />
      <div style={styles.bottomPane}>{bottom}</div>
    </div>
  );
}
