const pad = (value: number, width = 2): string =>
  String(value).padStart(width, "0");

/**
 * Format an epoch-millisecond timestamp as `YYYY-MM-DD HH:mm:ss.SSS`.
 *
 * Uses the browser's local timezone (the viewer's OS setting), so traffic
 * times always match the user's wall clock — unlike a string formatted by the
 * proxy process, which would follow whatever `TZ` that process was started
 * with.
 */
export function formatTimestamp(ms?: number | null): string {
  if (ms === undefined || ms === null || ms <= 0) return "-";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "-";
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}` +
    `.${pad(d.getMilliseconds(), 3)}`
  );
}
