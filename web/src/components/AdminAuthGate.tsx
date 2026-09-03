import { useEffect, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { Spin } from 'antd';

import {
  clearAdminToken,
  ensureAdminBrowserSession,
  fetchAdminAuthStatus,
} from '../services/adminAuth';

export default function AdminAuthGate({
  children,
}: {
  children: React.ReactNode;
}) {
  const [ready, setReady] = useState(false);
  const navigate = useNavigate();
  const location = useLocation();

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      let status;
      try {
        status = await fetchAdminAuthStatus();
      } catch {
        // Keep existing offline/startup behavior: inability to fetch status is
        // not enough evidence to force a login redirect.
        if (!cancelled) {
          setReady(true);
        }
        return;
      }

      if (cancelled) {
        return;
      }
      if (status.auth_required) {
        try {
          // Existing installations may already have a valid localStorage JWT
          // but no HttpOnly session cookie yet. Conversely, a valid HttpOnly
          // cookie can outlive cleared browser storage. Let the server accept
          // either credential before mounting native EventSource/WebSocket
          // channels.
          await ensureAdminBrowserSession();
        } catch {
          if (!cancelled) {
            clearAdminToken();
            const next = `${location.pathname}${location.search}`;
            navigate(`/login?next=${encodeURIComponent(next || '/traffic')}`, {
              replace: true,
            });
          }
          return;
        }
      }
      if (!cancelled) {
        setReady(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [location.pathname, location.search, navigate]);

  if (!ready) {
    return (
      <div style={{ display: 'grid', placeItems: 'center', height: '100vh' }}>
        <Spin />
      </div>
    );
  }
  return <>{children}</>;
}
