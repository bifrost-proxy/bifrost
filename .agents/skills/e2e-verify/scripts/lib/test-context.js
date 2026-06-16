const { SnapshotManager } = require("./snapshot-manager.js");

const DEFAULT_TIMEOUT = 5000;
const NAVIGATION_TIMEOUT = 30000;
const STABLE_ID_SYMBOL = Symbol("stableId");

class NetworkCollector {
  #requests = [];
  #idCounter = 1;
  #page = null;
  #requestHandler = null;
  #responseHandler = null;
  #failedHandler = null;

  attach(page) {
    this.#page = page;
    this.#requests = [];

    this.#requestHandler = (request) => {
      const entry = {
        id: this.#idCounter++,
        url: request.url(),
        method: request.method(),
        resourceType: request.resourceType(),
        timestamp: Date.now(),
        headers: request.headers(),
        postData: request.postData(),
        status: null,
        response: null,
        error: null,
        duration: null,
      };
      entry[STABLE_ID_SYMBOL] = entry.id;
      this.#requests.push(entry);
    };

    this.#responseHandler = (response) => {
      const request = response.request();
      const entry = this.#requests.find((r) => r.url === request.url() && r.status === null);
      if (entry) {
        entry.status = response.status();
        entry.response = {
          headers: response.headers(),
          statusText: response.statusText(),
        };
        entry.duration = Date.now() - entry.timestamp;
      }
    };

    this.#failedHandler = (request) => {
      const entry = this.#requests.find((r) => r.url === request.url() && r.status === null);
      if (entry) {
        entry.error = request.failure()?.errorText || "Request failed";
        entry.duration = Date.now() - entry.timestamp;
      }
    };

    page.on("request", this.#requestHandler);
    page.on("response", this.#responseHandler);
    page.on("requestfailed", this.#failedHandler);
  }

  detach() {
    if (this.#page) {
      this.#page.off("request", this.#requestHandler);
      this.#page.off("response", this.#responseHandler);
      this.#page.off("requestfailed", this.#failedHandler);
    }
    this.#page = null;
  }

  getRequests(filter = null) {
    if (!filter) {
      return [...this.#requests];
    }
    return this.#requests.filter(filter);
  }

  getRequestById(id) {
    const entry = this.#requests.find((r) => r.id === id);
    if (!entry) {
      throw new Error(`Network request with id ${id} not found.`);
    }
    return entry;
  }

  getRequestsByUrl(urlPattern) {
    const pattern = typeof urlPattern === "string" ? new RegExp(urlPattern) : urlPattern;
    return this.#requests.filter((r) => pattern.test(r.url));
  }

  getFailedRequests() {
    return this.#requests.filter((r) => r.error || (r.status && r.status >= 400));
  }

  clear() {
    this.#requests = [];
  }

  async waitForRequest(urlPattern, timeout = DEFAULT_TIMEOUT) {
    if (!this.#page) {
      throw new Error("NetworkCollector not attached to a page.");
    }

    const pattern = typeof urlPattern === "string" ? new RegExp(urlPattern) : urlPattern;

    return new Promise((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        cleanup();
        reject(new Error(`Timeout waiting for request matching ${urlPattern}`));
      }, timeout);

      const handler = (request) => {
        if (pattern.test(request.url())) {
          cleanup();
          resolve(request);
        }
      };

      const cleanup = () => {
        clearTimeout(timeoutId);
        this.#page.off("request", handler);
      };

      this.#page.on("request", handler);
    });
  }

  async waitForResponse(urlPattern, timeout = DEFAULT_TIMEOUT) {
    if (!this.#page) {
      throw new Error("NetworkCollector not attached to a page.");
    }

    const pattern = typeof urlPattern === "string" ? new RegExp(urlPattern) : urlPattern;

    return new Promise((resolve, reject) => {
      const timeoutId = setTimeout(() => {
        cleanup();
        reject(new Error(`Timeout waiting for response matching ${urlPattern}`));
      }, timeout);

      const handler = (response) => {
        if (pattern.test(response.url())) {
          cleanup();
          resolve(response);
        }
      };

      const cleanup = () => {
        clearTimeout(timeoutId);
        this.#page.off("response", handler);
      };

      this.#page.on("response", handler);
    });
  }
}

class ConsoleCollector {
  #messages = [];
  #idCounter = 1;
  #page = null;
  #consoleHandler = null;
  #errorHandler = null;

  attach(page) {
    this.#page = page;
    this.#messages = [];

    this.#consoleHandler = (message) => {
      const entry = {
        id: this.#idCounter++,
        type: message.type(),
        text: message.text(),
        location: message.location(),
        timestamp: Date.now(),
        args: message.args().map((arg) => arg.toString()),
      };
      entry[STABLE_ID_SYMBOL] = entry.id;
      this.#messages.push(entry);
    };

    this.#errorHandler = (error) => {
      const entry = {
        id: this.#idCounter++,
        type: "pageerror",
        text: error.message,
        stack: error.stack,
        timestamp: Date.now(),
      };
      entry[STABLE_ID_SYMBOL] = entry.id;
      this.#messages.push(entry);
    };

    page.on("console", this.#consoleHandler);
    page.on("pageerror", this.#errorHandler);
  }

  detach() {
    if (this.#page) {
      this.#page.off("console", this.#consoleHandler);
      this.#page.off("pageerror", this.#errorHandler);
    }
    this.#page = null;
  }

  getMessages(filter = null) {
    if (!filter) {
      return [...this.#messages];
    }
    return this.#messages.filter(filter);
  }

  getMessageById(id) {
    const entry = this.#messages.find((m) => m.id === id);
    if (!entry) {
      throw new Error(`Console message with id ${id} not found.`);
    }
    return entry;
  }

  getErrors() {
    return this.#messages.filter((m) => m.type === "error" || m.type === "pageerror");
  }

  getWarnings() {
    return this.#messages.filter((m) => m.type === "warning");
  }

  clear() {
    this.#messages = [];
  }
}

class TestContext {
  #browser = null;
  #page = null;
  #snapshotManager = new SnapshotManager();
  #networkCollector = new NetworkCollector();
  #consoleCollector = new ConsoleCollector();
  #defaultTimeout = DEFAULT_TIMEOUT;
  #navigationTimeout = NAVIGATION_TIMEOUT;
  #variables = new Map();
  #viewportConfig = null;
  #geolocationConfig = null;

  constructor(options = {}) {
    this.#defaultTimeout = options.timeout || DEFAULT_TIMEOUT;
    this.#navigationTimeout = options.navigationTimeout || NAVIGATION_TIMEOUT;
  }

  setBrowser(browser) {
    this.#browser = browser;
  }

  getBrowser() {
    if (!this.#browser) {
      throw new Error("No browser set. Call setBrowser() first.");
    }
    return this.#browser;
  }

  setPage(page) {
    if (this.#page) {
      this.#networkCollector.detach();
      this.#consoleCollector.detach();
    }

    this.#page = page;
    this.#snapshotManager.setPage(page);
    this.#networkCollector.attach(page);
    this.#consoleCollector.attach(page);

    page.setDefaultTimeout(this.#defaultTimeout);
    page.setDefaultNavigationTimeout(this.#navigationTimeout);
  }

  getPage() {
    if (!this.#page) {
      throw new Error("No page set. Call setPage() first.");
    }
    return this.#page;
  }

  get page() {
    return this.getPage();
  }

  getSnapshotManager() {
    return this.#snapshotManager;
  }

  getNetworkCollector() {
    return this.#networkCollector;
  }

  getConsoleCollector() {
    return this.#consoleCollector;
  }

  async createSnapshot(options = {}) {
    return this.#snapshotManager.createSnapshot(options);
  }

  async getOrCreateSnapshot(options = {}) {
    if (this.#snapshotManager.hasSnapshot()) {
      return this.#snapshotManager;
    }
    return this.#snapshotManager.createSnapshot(options);
  }

  async getElementByUid(uid) {
    return this.#snapshotManager.getHandleByUid(uid);
  }

  async getHandleByUid(uid) {
    return this.#snapshotManager.getHandleByUid(uid);
  }

  getElementNodeByUid(uid) {
    return this.#snapshotManager.getElementByUid(uid);
  }

  async waitForEventsAfterAction(action, options = {}) {
    const { timeout = this.#defaultTimeout } = options;
    const page = this.getPage();

    const initialRequestCount = this.#networkCollector.getRequests().length;

    await action();

    await page.waitForNetworkIdle({
      idleTime: 500,
      timeout: Math.min(timeout, 5000),
    }).catch(() => {});

    const finalRequestCount = this.#networkCollector.getRequests().length;
    const newRequests = finalRequestCount - initialRequestCount;

    return { newRequests };
  }

  async waitForText(text, timeout = this.#defaultTimeout) {
    const page = this.getPage();

    try {
      await page.waitForFunction(
        (searchText) => {
          return document.body?.innerText?.includes(searchText);
        },
        { timeout },
        text
      );
      return true;
    } catch (error) {
      throw new Error(
        `Text "${text}" not found on page within ${timeout}ms. ` +
        `Suggestion: Check if the page has fully loaded or if the text is dynamically rendered.`
      );
    }
  }

  async waitForElement(uid, timeout = this.#defaultTimeout) {
    const startTime = Date.now();

    while (Date.now() - startTime < timeout) {
      try {
        const element = await this.getElementByUid(uid);
        if (element) {
          return element;
        }
      } catch {}

      await new Promise((resolve) => setTimeout(resolve, 500));
      await this.createSnapshot();
    }

    const suggestions = this.#snapshotManager.getSuggestions(uid);
    let errorMsg = `Element with uid "${uid}" not found within ${timeout}ms.`;

    if (suggestions && suggestions.length > 0) {
      errorMsg += "\n\nSuggestions:\n";
      for (const s of suggestions) {
        errorMsg += `- ${s.message}\n`;
        if (s.elements) {
          for (const el of s.elements.slice(0, 5)) {
            errorMsg += `  - uid=${el.uid} ${el.role} "${el.name}"\n`;
          }
        }
      }
    }

    throw new Error(errorMsg);
  }

  setVariable(name, value) {
    this.#variables.set(name, value);
  }

  getVariable(name) {
    return this.#variables.get(name);
  }

  getVariables() {
    return new Map(this.#variables);
  }

  clearVariables() {
    this.#variables.clear();
  }

  async setViewport(width, height) {
    const page = this.getPage();
    this.#viewportConfig = { width, height };
    await page.setViewport({ width, height });
  }

  getViewport() {
    return this.#viewportConfig;
  }

  async setGeolocation(latitude, longitude) {
    const page = this.getPage();
    const browserContext = page.browserContext();
    this.#geolocationConfig = { latitude, longitude };

    await browserContext.overridePermissions(page.url(), ["geolocation"]);
    await page.setGeolocation({ latitude, longitude });
  }

  getGeolocation() {
    return this.#geolocationConfig;
  }

  async setOfflineMode(offline) {
    const page = this.getPage();
    await page.setOfflineMode(offline);
  }

  async emulateMediaFeatures(features) {
    const page = this.getPage();
    await page.emulateMediaFeatures(features);
  }

  async evaluateScript(expression) {
    const page = this.getPage();
    return page.evaluate(expression);
  }

  async screenshot(options = {}) {
    const page = this.getPage();
    const { uid, fullPage = false, path, type = "png", quality } = options;

    let screenshotOptions = { fullPage, type };
    if (path) screenshotOptions.path = path;
    if (quality && type !== "png") screenshotOptions.quality = quality;

    if (uid) {
      const element = await this.getHandleByUid(uid);
      return element.screenshot(screenshotOptions);
    }

    return page.screenshot(screenshotOptions);
  }

  dispose() {
    this.#networkCollector.detach();
    this.#consoleCollector.detach();
    this.#variables.clear();
    this.#page = null;
    this.#browser = null;
  }

  getStatus() {
    return {
      hasBrowser: !!this.#browser,
      hasPage: !!this.#page,
      hasSnapshot: this.#snapshotManager.hasSnapshot(),
      networkRequestCount: this.#networkCollector.getRequests().length,
      consoleMessageCount: this.#consoleCollector.getMessages().length,
      variableCount: this.#variables.size,
      viewport: this.#viewportConfig,
      geolocation: this.#geolocationConfig,
    };
  }
}

module.exports = {
  TestContext,
  NetworkCollector,
  ConsoleCollector,
};
