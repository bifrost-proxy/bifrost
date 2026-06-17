// Polyfill APIs that Monaco editor expects but jsdom does not provide.
if (
  typeof document !== "undefined" &&
  typeof document.queryCommandSupported !== "function"
) {
  document.queryCommandSupported = () => false;
}
