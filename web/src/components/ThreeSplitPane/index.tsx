import { useState, useRef, useCallback, useEffect, useMemo } from "react";
import type { ReactNode, CSSProperties } from "react";
import { theme } from "antd";
import { isMacDesktopShell } from "../../runtime";

interface ThreeSplitPaneProps {
  left?: ReactNode;
  center: ReactNode;
  right?: ReactNode;
  leftWidth: number;
  rightWidth?: string;
  minLeftWidth?: number;
  maxLeftWidth?: number;
  minCenterWidth?: number;
  minRightWidth?: number;
  leftCollapsed?: boolean;
  rightCollapsed?: boolean;
  keepRightMountedWhenCollapsed?: boolean;
  onLeftWidthChange?: (width: number) => void;
  onRightWidthChange?: (width: string) => void;
}

export default function ThreeSplitPane({
  left,
  center,
  right,
  leftWidth,
  rightWidth = "45%",
  minLeftWidth = 180,
  maxLeftWidth = 350,
  minCenterWidth = 400,
  minRightWidth = 350,
  leftCollapsed = false,
  rightCollapsed = false,
  keepRightMountedWhenCollapsed = false,
  onLeftWidthChange,
  onRightWidthChange,
}: ThreeSplitPaneProps) {
  const { token } = theme.useToken();
  const paneBackground = isMacDesktopShell() ? "transparent" : token.colorBgContainer;
  const containerRef = useRef<HTMLDivElement>(null);
  const [rightWidthPx, setRightWidthPx] = useState<string>(rightWidth);
  const [isDraggingLeft, setIsDraggingLeft] = useState(false);
  const [isDraggingRight, setIsDraggingRight] = useState(false);
  const [isHoveringLeft, setIsHoveringLeft] = useState(false);
  const [isHoveringRight, setIsHoveringRight] = useState(false);

  const styles = useMemo<Record<string, CSSProperties>>(
    () => ({
      container: {
        display: "flex",
        width: "100%",
        height: "100%",
        overflow: "hidden",
        backgroundColor: paneBackground,
      },
      leftPane: {
        height: "100%",
        overflow: "hidden",
        flexShrink: 0,
        backgroundColor: paneBackground,
      },
      centerPane: {
        flex: 1,
        height: "100%",
        overflow: "hidden",
        minWidth: 0,
        backgroundColor: paneBackground,
      },
      rightPane: {
        height: "100%",
        overflow: "auto",
        flexShrink: 0,
        backgroundColor: paneBackground,
      },
      divider: {
        width: 4,
        cursor: "col-resize",
        backgroundColor: token.colorBorderSecondary,
        flexShrink: 0,
        transition: "background-color 0.2s",
      },
      dividerHover: {
        backgroundColor: token.colorPrimary,
      },
    }),
    [paneBackground, token]
  );

  const handleLeftMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDraggingLeft(true);
  }, []);

  const handleRightMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDraggingRight(true);
  }, []);

  const handleMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!containerRef.current) return;
      const containerRect = containerRef.current.getBoundingClientRect();
      const containerWidth = containerRect.width;

      if (isDraggingLeft && !leftCollapsed) {
        const newLeftWidth = e.clientX - containerRect.left;
        const currentRightWidth = rightCollapsed ? 0 : parseRightWidth(rightWidthPx, containerWidth);
        const maxLeft = containerWidth - currentRightWidth - minCenterWidth - (rightCollapsed ? 4 : 8);

        if (newLeftWidth >= minLeftWidth && newLeftWidth <= Math.min(maxLeftWidth, maxLeft)) {
          onLeftWidthChange?.(newLeftWidth);
        }
      }

      if (isDraggingRight && !rightCollapsed) {
        const newRightWidth = containerRect.right - e.clientX;
        const currentLeftWidth = leftCollapsed ? 0 : leftWidth;
        const maxRight = containerWidth - currentLeftWidth - minCenterWidth - (leftCollapsed ? 4 : 8);

        if (newRightWidth >= minRightWidth && newRightWidth <= maxRight) {
          setRightWidthPx(`${newRightWidth}px`);
          onRightWidthChange?.(`${newRightWidth}px`);
        }
      }
    },
    [
      isDraggingLeft,
      isDraggingRight,
      leftCollapsed,
      rightCollapsed,
      leftWidth,
      rightWidthPx,
      minLeftWidth,
      maxLeftWidth,
      minCenterWidth,
      minRightWidth,
      onLeftWidthChange,
      onRightWidthChange,
    ]
  );

  const handleMouseUp = useCallback(() => {
    setIsDraggingLeft(false);
    setIsDraggingRight(false);
  }, []);

  useEffect(() => {
    if (isDraggingLeft || isDraggingRight) {
      document.addEventListener("mousemove", handleMouseMove);
      document.addEventListener("mouseup", handleMouseUp);
      document.body.style.cursor = "col-resize";
      document.body.style.userSelect = "none";
    }

    return () => {
      document.removeEventListener("mousemove", handleMouseMove);
      document.removeEventListener("mouseup", handleMouseUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, [isDraggingLeft, isDraggingRight, handleMouseMove, handleMouseUp]);

  const leftDividerStyle: CSSProperties = {
    ...styles.divider,
    ...(isHoveringLeft || isDraggingLeft ? styles.dividerHover : {}),
  };

  const rightDividerStyle: CSSProperties = {
    ...styles.divider,
    ...(isHoveringRight || isDraggingRight ? styles.dividerHover : {}),
  };
  const shouldRenderRightPane =
    !!right && (!rightCollapsed || keepRightMountedWhenCollapsed);

  return (
    <div ref={containerRef} style={styles.container}>
      {!leftCollapsed && left && (
        <>
          <div style={{ ...styles.leftPane, width: Math.max(leftWidth, minLeftWidth) }}>{left}</div>
          <div
            style={leftDividerStyle}
            onMouseDown={handleLeftMouseDown}
            onMouseEnter={() => setIsHoveringLeft(true)}
            onMouseLeave={() => setIsHoveringLeft(false)}
          />
        </>
      )}

      <div style={styles.centerPane}>{center}</div>

      {!rightCollapsed && right && (
        <>
          <div
            style={rightDividerStyle}
            onMouseDown={handleRightMouseDown}
            onMouseEnter={() => setIsHoveringRight(true)}
            onMouseLeave={() => setIsHoveringRight(false)}
          />
        </>
      )}
      {shouldRenderRightPane && (
        <div
          style={{
            ...styles.rightPane,
            width: rightCollapsed ? 0 : rightWidthPx,
            display: rightCollapsed ? "none" : "block",
          }}
        >
          {right}
        </div>
      )}
    </div>
  );
}

function parseRightWidth(width: string, containerWidth: number): number {
  if (width.endsWith("%")) {
    return (parseFloat(width) / 100) * containerWidth;
  }
  if (width.endsWith("px")) {
    return parseFloat(width);
  }
  return parseFloat(width) || 0;
}
