const fs = require("fs").promises;
const path = require("path");
const { VerifyLogger } = require("./logger");
const { SESSION_DIR, COOKIES_FILE, STORAGE_FILE } = require("./config");
const { ensureDir } = require("./browser-manager");

const logger = new VerifyLogger("SessionManager");
const METADATA_FILE = "metadata.json";

/**
 * @typedef {Object} SessionMetadata
 * @property {string} createdAt - ISO timestamp when session was created
 * @property {string} [sourceUrl] - URL where session was saved
 * @property {number} cookieCount - Number of cookies saved
 * @property {string[]} cookieDomains - List of unique cookie domains
 * @property {number} localStorageCount - Number of localStorage items
 * @property {number} sessionStorageCount - Number of sessionStorage items
 */

/**
 * @typedef {Object} SessionInfo
 * @property {boolean} exists - Whether the session exists
 * @property {string} sessionName - Session name
 * @property {boolean} hasCookies - Whether cookies file exists
 * @property {boolean} hasStorage - Whether storage file exists
 * @property {boolean} hasMetadata - Whether metadata file exists
 * @property {SessionMetadata} [metadata] - Session metadata if available
 * @property {number} [expiredCookieCount] - Number of expired cookies
 * @property {number} [validCookieCount] - Number of valid cookies
 */

/**
 * @typedef {Object} SaveResult
 * @property {boolean} success - Whether save was successful
 * @property {string} sessionName - Session name
 * @property {Object} [data] - Saved data (cookies and storage)
 * @property {string} [error] - Error message if failed
 */

/**
 * Filter out expired cookies from the list
 * @param {Object[]} cookies - Array of cookie objects
 * @returns {{valid: Object[], expired: Object[]}} Filtered cookies
 */
function filterExpiredCookies(cookies) {
  const now = Date.now() / 1000;
  const valid = [];
  const expired = [];

  for (const cookie of cookies) {
    if (cookie.expires && cookie.expires !== -1 && cookie.expires < now) {
      expired.push(cookie);
    } else {
      valid.push(cookie);
    }
  }

  return { valid, expired };
}

/**
 * Extract unique domains from cookies
 * @param {Object[]} cookies - Array of cookie objects
 * @returns {string[]} Unique domain list
 */
function extractCookieDomains(cookies) {
  const domains = new Set();
  for (const cookie of cookies) {
    if (cookie.domain) {
      domains.add(cookie.domain.replace(/^\./, ""));
    }
  }
  return Array.from(domains);
}

/**
 * Save browser session (cookies + storage) to local files
 * @param {import('puppeteer').Page} page - Puppeteer page instance
 * @param {string} [sessionName='default'] - Session name for identification
 * @returns {Promise<SaveResult>} Save result
 *
 * @example
 * // Save session after login
 * await page.goto('https://example.com/login');
 * // ... perform login ...
 * const result = await saveBrowserSession(page, 'my-session');
 * if (result.success) {
 *   console.log('Session saved with', result.data.cookies.length, 'cookies');
 * }
 */
async function saveBrowserSession(page, sessionName = "default") {
  const sessionDir = path.join(SESSION_DIR, sessionName);
  let client = null;

  try {
    await ensureDir(sessionDir);

    client = await page.target().createCDPSession();
    const cookiesResponse = await client.send("Network.getAllCookies");
    const cookies = cookiesResponse.cookies;

    await fs.writeFile(
      path.join(sessionDir, COOKIES_FILE),
      JSON.stringify(cookies, null, 2),
    );

    const storage = await page.evaluate(() => {
      const local = {};
      const session = {};
      for (let i = 0; i < localStorage.length; i++) {
        const key = localStorage.key(i);
        local[key] = localStorage.getItem(key);
      }
      for (let i = 0; i < sessionStorage.length; i++) {
        const key = sessionStorage.key(i);
        session[key] = sessionStorage.getItem(key);
      }
      return { localStorage: local, sessionStorage: session };
    });

    await fs.writeFile(
      path.join(sessionDir, STORAGE_FILE),
      JSON.stringify(storage, null, 2),
    );

    const sourceUrl = await page.url();
    const metadata = {
      createdAt: new Date().toISOString(),
      sourceUrl: sourceUrl !== "about:blank" ? sourceUrl : undefined,
      cookieCount: cookies.length,
      cookieDomains: extractCookieDomains(cookies),
      localStorageCount: Object.keys(storage.localStorage).length,
      sessionStorageCount: Object.keys(storage.sessionStorage).length,
    };

    await fs.writeFile(
      path.join(sessionDir, METADATA_FILE),
      JSON.stringify(metadata, null, 2),
    );

    logger.info(`Session saved: ${sessionName} (${cookies.length} cookies)`);
    return { success: true, sessionName, data: { cookies, storage, metadata } };
  } catch (error) {
    logger.error(`Failed to save session: ${sessionName}`, error.message);
    return { success: false, sessionName, error: error.message };
  } finally {
    if (client) {
      try {
        await client.detach();
      } catch {}
    }
  }
}

/**
 * Load browser session (cookies + storage) from local files
 * @param {import('puppeteer').Page} page - Puppeteer page instance
 * @param {string} [sessionName='default'] - Session name to load
 * @param {Object} [options] - Load options
 * @param {boolean} [options.skipExpired=true] - Skip expired cookies
 * @returns {Promise<{success: boolean, cookiesLoaded: number, expiredSkipped: number, storageLoaded: boolean}>}
 *
 * @example
 * // Load session before accessing protected page
 * const result = await loadBrowserSession(page, 'my-session');
 * if (result.success) {
 *   await page.goto('https://example.com/dashboard');
 * }
 */
async function loadBrowserSession(
  page,
  sessionName = "default",
  options = {},
) {
  const { skipExpired = true } = options;
  const sessionDir = path.join(SESSION_DIR, sessionName);
  const result = {
    success: false,
    cookiesLoaded: 0,
    expiredSkipped: 0,
    storageLoaded: false,
  };

  let client = null;
  try {
    const cookiesPath = path.join(sessionDir, COOKIES_FILE);
    const cookiesData = await fs.readFile(cookiesPath, "utf8");
    let cookies = JSON.parse(cookiesData);

    if (skipExpired) {
      const { valid, expired } = filterExpiredCookies(cookies);
      result.expiredSkipped = expired.length;
      cookies = valid;
      if (expired.length > 0) {
        logger.info(`Skipped ${expired.length} expired cookies`);
      }
    }

    client = await page.target().createCDPSession();
    await client.send("Network.setCookies", { cookies });
    result.cookiesLoaded = cookies.length;
    logger.info(`Loaded ${cookies.length} cookies`);
  } catch (e) {
    if (e.code !== "ENOENT") {
      logger.warn(`Failed to load cookies: ${e.message}`);
    }
  } finally {
    if (client) {
      try {
        await client.detach();
      } catch {}
    }
  }

  try {
    const storagePath = path.join(sessionDir, STORAGE_FILE);
    const storageData = await fs.readFile(storagePath, "utf8");
    const storage = JSON.parse(storageData);

    await page.evaluate((data) => {
      if (data.localStorage) {
        Object.entries(data.localStorage).forEach(([k, v]) =>
          localStorage.setItem(k, v),
        );
      }
      if (data.sessionStorage) {
        Object.entries(data.sessionStorage).forEach(([k, v]) =>
          sessionStorage.setItem(k, v),
        );
      }
    }, storage);
    result.storageLoaded = true;
    logger.info("Loaded storage data");
  } catch (e) {
    if (e.code !== "ENOENT") {
      logger.warn(`Failed to load storage: ${e.message}`);
    }
  }

  result.success = result.cookiesLoaded > 0 || result.storageLoaded;
  return result;
}

/**
 * Check if a session exists (has cookies OR storage file)
 * @param {string} [sessionName='default'] - Session name to check
 * @returns {Promise<boolean>} True if session exists
 *
 * @example
 * if (await sessionExists('my-session')) {
 *   await loadBrowserSession(page, 'my-session');
 * }
 */
async function sessionExists(sessionName = "default") {
  const sessionDir = path.join(SESSION_DIR, sessionName);
  try {
    const cookiesExists = await fs
      .access(path.join(sessionDir, COOKIES_FILE))
      .then(() => true)
      .catch(() => false);
    const storageExists = await fs
      .access(path.join(sessionDir, STORAGE_FILE))
      .then(() => true)
      .catch(() => false);
    return cookiesExists || storageExists;
  } catch {
    return false;
  }
}

/**
 * Get detailed session information including metadata and cookie status
 * @param {string} [sessionName='default'] - Session name to inspect
 * @returns {Promise<SessionInfo>} Detailed session information
 *
 * @example
 * const info = await getSessionInfo('my-session');
 * if (info.exists && info.expiredCookieCount > 0) {
 *   console.log(`Warning: ${info.expiredCookieCount} cookies expired`);
 * }
 */
async function getSessionInfo(sessionName = "default") {
  const sessionDir = path.join(SESSION_DIR, sessionName);
  const info = {
    exists: false,
    sessionName,
    hasCookies: false,
    hasStorage: false,
    hasMetadata: false,
  };

  try {
    info.hasCookies = await fs
      .access(path.join(sessionDir, COOKIES_FILE))
      .then(() => true)
      .catch(() => false);

    info.hasStorage = await fs
      .access(path.join(sessionDir, STORAGE_FILE))
      .then(() => true)
      .catch(() => false);

    info.hasMetadata = await fs
      .access(path.join(sessionDir, METADATA_FILE))
      .then(() => true)
      .catch(() => false);

    info.exists = info.hasCookies || info.hasStorage;

    if (info.hasMetadata) {
      const metadataPath = path.join(sessionDir, METADATA_FILE);
      const metadataData = await fs.readFile(metadataPath, "utf8");
      info.metadata = JSON.parse(metadataData);
    }

    if (info.hasCookies) {
      const cookiesPath = path.join(sessionDir, COOKIES_FILE);
      const cookiesData = await fs.readFile(cookiesPath, "utf8");
      const cookies = JSON.parse(cookiesData);
      const { valid, expired } = filterExpiredCookies(cookies);
      info.validCookieCount = valid.length;
      info.expiredCookieCount = expired.length;
    }
  } catch {}

  return info;
}

/**
 * Delete a saved session
 * @param {string} [sessionName='default'] - Session name to delete
 * @returns {Promise<boolean>} True if deletion was successful
 *
 * @example
 * await deleteSession('old-session');
 */
async function deleteSession(sessionName = "default") {
  const sessionDir = path.join(SESSION_DIR, sessionName);
  try {
    await fs.rm(sessionDir, { recursive: true });
    logger.info(`Session deleted: ${sessionName}`);
    return true;
  } catch {
    return false;
  }
}

/**
 * List all saved sessions
 * @returns {Promise<string[]>} Array of session names
 *
 * @example
 * const sessions = await listSessions();
 * console.log('Available sessions:', sessions);
 */
async function listSessions() {
  try {
    const entries = await fs.readdir(SESSION_DIR, { withFileTypes: true });
    return entries.filter((e) => e.isDirectory()).map((e) => e.name);
  } catch {
    return [];
  }
}

module.exports = {
  saveBrowserSession,
  loadBrowserSession,
  sessionExists,
  getSessionInfo,
  deleteSession,
  listSessions,
};
