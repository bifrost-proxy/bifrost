const path = require('path');

const SKILL_ROOT = path.resolve(__dirname, '../../');
const REPO_ROOT = path.resolve(__dirname, '../../../../../');
const ONCALL_ROOT = REPO_ROOT;

const SCRIPTS_ROOT = path.resolve(__dirname, '../');
const SESSION_DIR = path.join(REPO_ROOT, '.env/browser-sessions');
const SCREENSHOT_DIR = path.join(SKILL_ROOT, 'screenshots');
const LOGS_DIR = path.join(SKILL_ROOT, 'logs');
const SCENARIOS_DIR = path.join(SCRIPTS_ROOT, 'scenarios');
const COOKIE_DIR = path.join(REPO_ROOT, '.env');
const CONFIG_FILE = path.join(REPO_ROOT, '.env/.app_config');

const SHARED_PROFILE_DIR = path.join(SESSION_DIR, 'profile-shared');
const FRONTEND_ROOT = path.join(REPO_ROOT, 'web');
const ONCALL_ADMIN_SRC = path.join(FRONTEND_ROOT, 'src');

const COOKIES_FILE = 'cookies.json';
const STORAGE_FILE = 'storage.json';

const DEFAULT_PORT = 9900;
const DEFAULT_UI_URL = 'http://localhost:3000/_bifrost/';
const DEFAULT_APP_ID = 'appId';
const DEFAULT_TIMEOUT = 30000;
const DEFAULT_SESSION = 'default';

const COOKIE_DOMAIN = {
  file: 'localhost',
  domain: 'localhost',
};

module.exports = {
  SKILL_ROOT,
  REPO_ROOT,
  ONCALL_ROOT,
  SCRIPTS_ROOT,
  SESSION_DIR,
  SCREENSHOT_DIR,
  LOGS_DIR,
  SCENARIOS_DIR,
  COOKIE_DIR,
  CONFIG_FILE,
  SHARED_PROFILE_DIR,
  FRONTEND_ROOT,
  ONCALL_ADMIN_SRC,
  COOKIES_FILE,
  STORAGE_FILE,
  DEFAULT_PORT,
  DEFAULT_UI_URL,
  DEFAULT_APP_ID,
  DEFAULT_TIMEOUT,
  DEFAULT_SESSION,
  COOKIE_DOMAIN,
};
