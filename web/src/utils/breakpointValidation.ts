export function isFinalBreakpointStatus(status: number | undefined): boolean {
  return (
    status !== undefined &&
    Number.isInteger(status) &&
    status >= 200 &&
    status <= 599
  );
}
