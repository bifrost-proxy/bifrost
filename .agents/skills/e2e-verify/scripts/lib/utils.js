const fs = require('fs');
const path = require('path');
const http = require('http');
const { execSync } = require('child_process');
const {
  CONFIG_FILE,
  ONCALL_ROOT,
  DEFAULT_PORT,
  DEFAULT_APP_ID,
  DEFAULT_TIMEOUT,
  DEFAULT_SESSION,
} = require('./config');

function loadState(branchName) {
  const stateDir = path.join(ONCALL_ROOT, '.env');
  const stateFile = path.join(stateDir, `${branchName}.json`);
  if (!fs.existsSync(stateFile)) return null;
  try {
    return JSON.parse(fs.readFileSync(stateFile, 'utf-8'));
  } catch {
    return null;
  }
}

function loadAppConfig() {
  if (!fs.existsSync(CONFIG_FILE)) return {};
  try {
    return JSON.parse(fs.readFileSync(CONFIG_FILE, 'utf-8'));
  } catch {
    return {};
  }
}

function getAppId() {
  const config = loadAppConfig();
  return config.appId || DEFAULT_APP_ID;
}

function checkServerRunning(port = DEFAULT_PORT) {
  return new Promise((resolve) => {
    const req = http.request(
      {
        hostname: 'localhost',
        port,
        path: '/',
        method: 'HEAD',
        timeout: 2000,
      },
      (res) => {
        resolve(true);
      }
    );
    req.on('error', () => resolve(false));
    req.on('timeout', () => {
      req.destroy();
      resolve(false);
    });
    req.end();
  });
}

function getEdgePath() {
  const possiblePaths = [
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    '/usr/bin/microsoft-edge',
    '/usr/bin/microsoft-edge-stable',
    'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe',
    'C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe',
  ];

  for (const p of possiblePaths) {
    if (fs.existsSync(p)) {
      return p;
    }
  }

  try {
    const result = execSync(
      'which microsoft-edge 2>/dev/null || which msedge 2>/dev/null',
      {
        encoding: 'utf-8',
      }
    ).trim();
    if (result) return result;
  } catch {}

  return null;
}

function replaceAppId(url, appId) {
  if (!url || !appId) return url;
  return url.replace(/:appId\b/g, appId);
}

module.exports = {
  loadState,
  loadAppConfig,
  getAppId,
  checkServerRunning,
  getEdgePath,
  replaceAppId,
  DEFAULT_PORT,
  DEFAULT_APP_ID,
  DEFAULT_TIMEOUT,
  DEFAULT_SESSION,
};
