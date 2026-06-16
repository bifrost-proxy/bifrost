const fs = require('fs');
const path = require('path');
const { LOGS_DIR } = require('./config');

const COLORS = {
  reset: '\x1b[0m',
  bright: '\x1b[1m',
  dim: '\x1b[2m',
  red: '\x1b[31m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  blue: '\x1b[34m',
  magenta: '\x1b[35m',
  cyan: '\x1b[36m',
  gray: '\x1b[90m',
};

class VerifyLogger {
  constructor(name = 'E2E') {
    this.name = name;
    this.logs = {
      network: [],
      console: [],
      components: [],
      errors: [],
    };
    this.startTime = Date.now();
  }

  _format(level, message, ...args) {
    const timestamp = new Date().toISOString().substring(11, 23);
    const colorMap = {
      info: COLORS.blue,
      warn: COLORS.yellow,
      error: COLORS.red,
      debug: COLORS.gray,
      success: COLORS.green,
    };
    const color = colorMap[level] || COLORS.reset;
    const levelStr = level.toUpperCase().padEnd(5);
    
    let formatted = `${COLORS.dim}[${timestamp}]${COLORS.reset} ${color}${levelStr}${COLORS.reset} ${COLORS.cyan}[${this.name}]${COLORS.reset} ${message}`;
    
    if (args.length > 0) {
      const extra = args.map(a => typeof a === 'object' ? JSON.stringify(a) : a).join(' ');
      formatted += ` ${COLORS.dim}${extra}${COLORS.reset}`;
    }
    
    return formatted;
  }

  info(message, ...args) {
    console.log(this._format('info', message, ...args));
  }

  warn(message, ...args) {
    console.warn(this._format('warn', message, ...args));
  }

  error(message, ...args) {
    console.error(this._format('error', message, ...args));
  }

  debug(message, ...args) {
    if (process.env.DEBUG) {
      console.log(this._format('debug', message, ...args));
    }
  }

  success(message, ...args) {
    console.log(this._format('success', message, ...args));
  }

  logNetwork(entry) {
    this.logs.network.push({
      timestamp: Date.now(),
      ...entry,
    });
  }

  logConsole(entry) {
    this.logs.console.push({
      timestamp: Date.now(),
      ...entry,
    });
  }

  logComponent(entry) {
    this.logs.components.push({
      timestamp: Date.now(),
      ...entry,
    });
  }

  logError(entry) {
    this.logs.errors.push({
      timestamp: Date.now(),
      ...entry,
    });
  }

  getNetworkLogs() {
    return this.logs.network;
  }

  getConsoleLogs() {
    return this.logs.console;
  }

  getComponentLogs() {
    return this.logs.components;
  }

  getErrorLogs() {
    return this.logs.errors;
  }

  getSummary() {
    const duration = Date.now() - this.startTime;
    const networkErrors = this.logs.network.filter(
      (n) => n.status >= 400 || n.failed
    );
    const consoleErrors = this.logs.console.filter(
      (c) => c.type === 'error' || c.type === 'pageerror'
    );

    return {
      duration,
      totalNetworkRequests: this.logs.network.length,
      networkErrors: networkErrors.length,
      consoleErrors: consoleErrors.length,
      componentChecks: this.logs.components.length,
      errors: this.logs.errors.length,
    };
  }

  formatTimestamp(ts) {
    const date = new Date(ts);
    return date.toISOString().replace('T', ' ').substring(0, 23);
  }

  printNetworkSummary(limit = 20) {
    const logs = this.logs.network;
    if (logs.length === 0) {
      console.log('   📊 无网络请求记录');
      return;
    }

    console.log(`   📊 网络请求 (共 ${logs.length} 个):`);

    const errors = logs.filter((l) => l.status >= 400 || l.failed);
    const slow = logs.filter((l) => l.duration && l.duration > 3000);
    const apiRequests = logs.filter(
      (l) =>
        l.url && (l.url.includes('/api/') || l.url.includes('/gateway/'))
    );

    if (errors.length > 0) {
      console.log(`      ❌ 错误请求: ${errors.length}`);
      errors.slice(0, 5).forEach((e) => {
        const url = e.url?.length > 60 ? e.url.substring(0, 60) + '...' : e.url;
        console.log(`         - [${e.status || 'FAILED'}] ${url}`);
      });
    }

    if (slow.length > 0) {
      console.log(`      🐌 慢请求 (>3s): ${slow.length}`);
      slow.slice(0, 3).forEach((s) => {
        const url = s.url?.length > 50 ? s.url.substring(0, 50) + '...' : s.url;
        console.log(`         - ${s.duration}ms ${url}`);
      });
    }

    if (apiRequests.length > 0) {
      console.log(`      🔌 API 请求: ${apiRequests.length}`);
      const byEndpoint = {};
      apiRequests.forEach((r) => {
        try {
          const urlObj = new URL(r.url);
          const endpoint = urlObj.pathname;
          if (!byEndpoint[endpoint]) byEndpoint[endpoint] = [];
          byEndpoint[endpoint].push(r);
        } catch {}
      });
      Object.entries(byEndpoint)
        .slice(0, 5)
        .forEach(([endpoint, reqs]) => {
          const avgDuration =
            reqs.reduce((sum, r) => sum + (r.duration || 0), 0) / reqs.length;
          console.log(
            `         - ${endpoint} (${reqs.length}次, 平均${Math.round(avgDuration)}ms)`
          );
        });
    }
  }

  printConsoleSummary() {
    const logs = this.logs.console;
    if (logs.length === 0) {
      console.log('   📋 无控制台输出');
      return;
    }

    const errors = logs.filter(
      (l) => l.type === 'error' || l.type === 'pageerror'
    );
    const warnings = logs.filter((l) => l.type === 'warning');

    console.log(`   📋 控制台输出 (共 ${logs.length} 条):`);

    if (errors.length > 0) {
      console.log(`      ❌ 错误: ${errors.length}`);
      errors.slice(0, 5).forEach((e) => {
        const text =
          e.text?.length > 80 ? e.text.substring(0, 80) + '...' : e.text;
        console.log(`         - ${text}`);
      });
    }

    if (warnings.length > 0) {
      console.log(`      ⚠️  警告: ${warnings.length}`);
    }
  }

  saveToFile(filename) {
    if (!fs.existsSync(LOGS_DIR)) {
      fs.mkdirSync(LOGS_DIR, { recursive: true });
    }

    const filepath = path.join(LOGS_DIR, filename);
    const data = {
      startTime: this.startTime,
      endTime: Date.now(),
      summary: this.getSummary(),
      logs: this.logs,
    };

    fs.writeFileSync(filepath, JSON.stringify(data, null, 2));
    return filepath;
  }

  clear() {
    this.logs = {
      network: [],
      console: [],
      components: [],
      errors: [],
    };
    this.startTime = Date.now();
  }
}

module.exports = { VerifyLogger };
