import { useState, useEffect, useRef, type CSSProperties } from "react";
import { Avatar } from "antd";
import { AppstoreOutlined } from "@ant-design/icons";
import { apiFetch } from "../../api/apiFetch";

interface AppIconProps {
  appName: string;
  size?: number;
  style?: CSSProperties;
}

const iconCache = new Map<string, string | null>();
const pendingRequests = new Map<string, Promise<string | null>>();
const MAX_ACTIVE_ICON_REQUESTS = 4;

let activeIconRequests = 0;
const queuedIconRequests: Array<() => void> = [];

const runNextIconRequest = () => {
  if (activeIconRequests >= MAX_ACTIVE_ICON_REQUESTS) {
    return;
  }

  const next = queuedIconRequests.shift();
  if (next) {
    next();
  }
};

const enqueueIconRequest = <T,>(task: () => Promise<T>): Promise<T> => {
  return new Promise((resolve, reject) => {
    const run = () => {
      activeIconRequests += 1;
      task()
        .then(resolve, reject)
        .finally(() => {
          activeIconRequests = Math.max(0, activeIconRequests - 1);
          runNextIconRequest();
        });
    };

    if (activeIconRequests < MAX_ACTIVE_ICON_REQUESTS) {
      run();
    } else {
      queuedIconRequests.push(run);
    }
  });
};

const fetchAppIcon = async (appName: string): Promise<string | null> => {
  return enqueueIconRequest(async () => {
    try {
      const encodedName = encodeURIComponent(appName);
      const response = await apiFetch(`/_bifrost/api/app-icon/${encodedName}`);

      if (response.ok) {
        const blob = await response.blob();
        const url = URL.createObjectURL(blob);
        return url;
      }
      return null;
    } catch {
      return null;
    }
  });
};

export const AppIcon: React.FC<AppIconProps> = ({ appName, size = 16, style }) => {
  const rootRef = useRef<HTMLSpanElement | null>(null);
  const [isVisible, setIsVisible] = useState(() => {
    return typeof IntersectionObserver === "undefined";
  });
  const [iconUrl, setIconUrl] = useState<string | null>(() => {
    return iconCache.get(appName) ?? null;
  });
  const [loading, setLoading] = useState(() => {
    return false;
  });
  const [error, setError] = useState(() => {
    const cached = iconCache.get(appName);
    return cached === null || !appName;
  });

  useEffect(() => {
    if (isVisible) {
      return;
    }

    const element = rootRef.current;
    if (!element || typeof IntersectionObserver === "undefined") {
      setIsVisible(true);
      return;
    }

    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) {
          setIsVisible(true);
          observer.disconnect();
        }
      },
      { rootMargin: "160px" },
    );

    observer.observe(element);
    return () => observer.disconnect();
  }, [isVisible]);

  useEffect(() => {
    if (!appName) {
      setLoading(false);
      setError(true);
      return;
    }

    if (iconCache.has(appName)) {
      const cached = iconCache.get(appName);
      setIconUrl(cached ?? null);
      setLoading(false);
      setError(cached === null);
      return;
    }

    if (!isVisible) {
      setLoading(false);
      return;
    }

    setIconUrl(null);
    setError(false);
    setLoading(true);

    let cancelled = false;

    const loadIcon = async () => {
      let promise = pendingRequests.get(appName);
      
      if (!promise) {
        promise = fetchAppIcon(appName);
        pendingRequests.set(appName, promise);
      }

      try {
        const url = await promise;
        iconCache.set(appName, url);
        
        if (!cancelled) {
          setIconUrl(url);
          setError(url === null);
          setLoading(false);
        }
      } finally {
        pendingRequests.delete(appName);
      }
    };

    loadIcon();

    return () => {
      cancelled = true;
    };
  }, [appName, isVisible]);

  if (loading || error || !iconUrl) {
    return (
      <span ref={rootRef} style={{ display: "inline-flex", width: size, height: size }}>
        <Avatar
          size={size}
          icon={<AppstoreOutlined />}
          style={{
            backgroundColor: "#f0f0f0",
            color: "#999",
            fontSize: size * 0.6,
            ...style,
          }}
        />
      </span>
    );
  }

  return (
    <span ref={rootRef} style={{ display: "inline-flex", width: size, height: size }}>
      <Avatar
        size={size}
        src={iconUrl}
        style={{ backgroundColor: "transparent", ...style }}
      />
    </span>
  );
};

export default AppIcon;
