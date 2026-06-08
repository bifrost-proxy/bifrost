export function normalizeClientIp(ip: string): string {
  return ip.trim().replace(/^\[|\]$/g, "").toLowerCase();
}

export function isLoopbackClientIp(ip: string): boolean {
  const normalized = normalizeClientIp(ip);
  if (
    normalized === "localhost" ||
    normalized === "::1" ||
    normalized === "0:0:0:0:0:0:0:1"
  ) {
    return true;
  }

  const mappedIpv4 = normalized.startsWith("::ffff:")
    ? normalized.slice("::ffff:".length)
    : normalized;

  return /^127(?:\.\d{1,3}){3}$/.test(mappedIpv4);
}

export function isClientIpWhitelisted(ip: string, whitelist: string[] | undefined): boolean {
  const normalized = normalizeClientIp(ip);
  return (whitelist ?? []).some((entry) => {
    const normalizedEntry = normalizeClientIp(entry);
    if (normalizedEntry === normalized) {
      return true;
    }
    if (normalizedEntry.endsWith("/32") && normalizedEntry.slice(0, -3) === normalized) {
      return true;
    }
    if (normalizedEntry.endsWith("/128") && normalizedEntry.slice(0, -4) === normalized) {
      return true;
    }
    return false;
  });
}
