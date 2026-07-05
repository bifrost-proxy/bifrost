import { useState, useRef, useCallback, useEffect } from "react";
import type { ReactNode, CSSProperties } from "react";
import { theme } from "antd";
import { isMacDesktopShell } from "../../runtime";

interface SplitPaneProps {
  left: ReactNode;
  right: ReactNode;
  defaultLeftWidth?: string;
  minLeftWidth?: number;
  minRightWidth?: number;
  transparentLeftOnMacDesktop?: boolean;
}

export default function SplitPane({
  left,
  right,
  defaultLeftWidth = "60%",
  minLeftWidth = 200,
  minRightWidth = 300,
  transparentLeftOnMacDesktop = false,
}: SplitPaneProps) {
  const { token } = theme.useToken();
  const macLeftTransparency = transparentLeftOnMacDesktop && isMacDesktopShell();
  const paneBackground = token.colorBgContainer;
  const containerRef = useRef<HTMLDivElement>(null);
  const [leftWidth, setLeftWidth] = useState<string>(defaultLeftWidth);
  const [isDragging, setIsDragging] = useState(false);
  const [isHovering, setIsHovering] = useState(false);

  const styles: Record<string, CSSProperties> = {
    container: {
      display: "flex",
      width: "100%",
      height: "100%",
      overflow: "hidden",
      backgroundColor: macLeftTransparency ? "transparent" : paneBackground,
    },
    leftPane: {
      height: "100%",
      overflow: "auto",
      flexShrink: 0,
      backgroundColor: macLeftTransparency ? "transparent" : paneBackground,
    },
    rightPane: {
      flex: 1,
      height: "100%",
      overflow: "auto",
      minWidth: 0,
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
  };

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  const handleMouseMove = useCallback(
    (e: MouseEvent) => {
      if (!isDragging || !containerRef.current) return;

      const containerRect = containerRef.current.getBoundingClientRect();
      const containerWidth = containerRect.width;
      const newLeftWidth = e.clientX - containerRect.left;

      const maxLeftWidth = containerWidth - minRightWidth - 4;

      if (newLeftWidth >= minLeftWidth && newLeftWidth <= maxLeftWidth) {
        setLeftWidth(`${newLeftWidth}px`);
      }
    },
    [isDragging, minLeftWidth, minRightWidth],
  );

  const handleMouseUp = useCallback(() => {
    setIsDragging(false);
  }, []);

  useEffect(() => {
    if (isDragging) {
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
  }, [isDragging, handleMouseMove, handleMouseUp]);

  const dividerStyle: CSSProperties = {
    ...styles.divider,
    ...(isHovering || isDragging ? styles.dividerHover : {}),
  };

  return (
    <div ref={containerRef} style={styles.container}>
      <div style={{ ...styles.leftPane, width: leftWidth }}>{left}</div>
      <div
        style={dividerStyle}
        onMouseDown={handleMouseDown}
        onMouseEnter={() => setIsHovering(true)}
        onMouseLeave={() => setIsHovering(false)}
      />
      <div style={styles.rightPane}>{right}</div>
    </div>
  );
}
