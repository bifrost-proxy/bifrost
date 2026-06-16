class NetworkCollector {
  #requests = [];
  #responses = new Map();
  #page = null;
  #enabled = false;
  #startTime = 0;
  #filter = null;

  constructor(options = {}) {
    this.#filter = options.filter || null;
  }

  attach(page) {
    this.#page = page;
    this.#startTime = Date.now();

    page.on("request", (request) => {
      if (!this.#enabled) return;

      const url = request.url();
      if (this.#shouldIgnore(url)) return;

      const entry = {
        id: this.#requests.length,
        timestamp: Date.now() - this.#startTime,
        method: request.method(),
        url: this.#shortenUrl(url),
        fullUrl: url,
        resourceType: request.resourceType(),
        status: null,
        duration: null,
        size: null,
        error: null,
        startTime: Date.now(),
      };

      this.#requests.push(entry);
    });

    page.on("response", (response) => {
      if (!this.#enabled) return;

      const url = response.url();
      if (this.#shouldIgnore(url)) return;

      const entry = this.#requests.find(
        (r) => r.fullUrl === url && r.status === null,
      );

      if (entry) {
        entry.status = response.status();
        entry.duration = Date.now() - entry.startTime;
        entry.statusText = response.statusText();

        const contentLength = response.headers()["content-length"];
        if (contentLength) {
          entry.size = parseInt(contentLength, 10);
        }
      }
    });

    page.on("requestfailed", (request) => {
      if (!this.#enabled) return;

      const url = request.url();
      const entry = this.#requests.find(
        (r) => r.fullUrl === url && r.status === null,
      );

      if (entry) {
        entry.status = "FAILED";
        entry.error = request.failure()?.errorText || "Unknown error";
        entry.duration = Date.now() - entry.startTime;
      }
    });

    page.on("requestfinished", (request) => {
      if (!this.#enabled) return;

      const url = request.url();
      const entry = this.#requests.find(
        (r) => r.fullUrl === url && r.duration === null,
      );

      if (entry && entry.duration === null) {
        entry.duration = Date.now() - entry.startTime;
      }
    });
  }

  #shouldIgnore(url) {
    const ignorePatterns = [
      /\.(js|css|png|jpg|jpeg|gif|svg|ico|woff|woff2|ttf|eot)(\?|$)/i,
      /^data:/,
      /^chrome-extension:/,
      /\/sockjs-node\//,
      /\/hot-update\./,
      /__webpack_hmr/,
      /webpack-dev-server/,
    ];

    if (this.#filter && !this.#filter(url)) {
      return true;
    }

    return ignorePatterns.some((p) => p.test(url));
  }

  #shortenUrl(url) {
    try {
      const u = new URL(url);
      const path = u.pathname + u.search;
      if (path.length > 60) {
        return path.substring(0, 57) + "...";
      }
      return path;
    } catch {
      return url.length > 60 ? url.substring(0, 57) + "..." : url;
    }
  }

  start() {
    this.#enabled = true;
    this.#startTime = Date.now();
  }

  stop() {
    this.#enabled = false;
  }

  clear() {
    this.#requests = [];
    this.#responses.clear();
    this.#startTime = Date.now();
  }

  getRequests() {
    return [...this.#requests];
  }

  getRequestsSince(timestamp) {
    return this.#requests.filter((r) => r.startTime >= timestamp);
  }

  getApiRequests() {
    return this.#requests.filter(
      (r) =>
        r.resourceType === "xhr" ||
        r.resourceType === "fetch" ||
        r.url.includes("/api/") ||
        r.url.includes("/v1/") ||
        r.url.includes("/v2/"),
    );
  }

  getFailedRequests() {
    return this.#requests.filter(
      (r) =>
        r.status === "FAILED" ||
        (r.status !== null && r.status >= 400),
    );
  }

  formatRequest(req) {
    const status = req.status === null ? "..." : req.status;
    const duration = req.duration !== null ? `${req.duration}ms` : "...";
    const size = req.size ? this.#formatSize(req.size) : "";

    let statusColor = "";
    let resetColor = "\x1b[0m";

    if (req.status === "FAILED" || (req.status && req.status >= 400)) {
      statusColor = "\x1b[31m"; // red
    } else if (req.status && req.status >= 300) {
      statusColor = "\x1b[33m"; // yellow
    } else if (req.status && req.status >= 200) {
      statusColor = "\x1b[32m"; // green
    }

    const statusStr = `${statusColor}${status}${resetColor}`;
    const sizeStr = size ? ` [${size}]` : "";

    return `${req.method.padEnd(6)} ${statusStr} ${req.url} (${duration})${sizeStr}`;
  }

  #formatSize(bytes) {
    if (bytes < 1024) return `${bytes}B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)}KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  }

  formatRequestsTable(requests = null) {
    const reqs = requests || this.getApiRequests();

    if (reqs.length === 0) {
      return "   (无 API 请求)";
    }

    const lines = reqs.map((req) => `   ${this.formatRequest(req)}`);
    return lines.join("\n");
  }

  getSummary() {
    const total = this.#requests.length;
    const api = this.getApiRequests().length;
    const failed = this.getFailedRequests().length;

    return {
      total,
      api,
      failed,
      requests: this.#requests,
    };
  }
}

module.exports = { NetworkCollector };
