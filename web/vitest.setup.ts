// Polyfill APIs that Monaco editor expects but jsdom does not provide.
if (typeof document.queryCommandSupported !== "function") {
  document.queryCommandSupported = () => false;
}
