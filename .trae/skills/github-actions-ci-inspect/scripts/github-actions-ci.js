#!/usr/bin/env node

const fs = require("fs");
const path = require("path");

const GITHUB_BASE = "https://github.com";
const DEFAULT_CONFIG = {
  repo: "bifrost-proxy/bifrost",
  workflow: "ci.yml",
  cookieFile: ".env/.cookie.github.com",
  run: "latest",
  format: "text",
  fetchLogs: false,
  failedOnly: false,
  maxLogLines: 80,
  logExcerptLines: 40,
  logContextLines: 50,
  watch: true,
  job: null,
  pollInterval: 5000,
};

function parseArgs(argv) {
  const args = {};
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    if (!key.startsWith("--")) {
      continue;
    }
    const name = key.slice(2);
    const next = argv[i + 1];
    if (!next || next.startsWith("--")) {
      args[name] = true;
      continue;
    }
    args[name] = next;
    i += 1;
  }
  return args;
}

function decodeHtml(value) {
  return String(value || "")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&apos;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&")
    .replace(/&nbsp;/g, " ");
}

function stripTags(value) {
  return decodeHtml(String(value || ""))
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/div>/gi, "\n")
    .replace(/<\/p>/gi, "\n")
    .replace(/<[^>]+>/g, "")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
}

function loadJsonIfExists(filePath) {
  if (!fs.existsSync(filePath)) {
    return {};
  }
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function readCookieFile(filePath) {
  if (!fs.existsSync(filePath)) {
    throw new Error(`Cookie 文件不存在: ${filePath}`);
  }
  const text = fs.readFileSync(filePath, "utf8").trim();
  if (!text) {
    throw new Error(`Cookie 文件为空: ${filePath}`);
  }
  return text;
}

function mergeConfig(args) {
  const repoRoot = process.cwd();
  const configPath = path.resolve(repoRoot, args.config || ".env/github-actions-ci.json");
  const fileConfig = loadJsonIfExists(configPath);
  const config = {
    ...DEFAULT_CONFIG,
    ...fileConfig,
  };
  if (args.repo) {
    config.repo = args.repo;
  }
  if (args.workflow) {
    config.workflow = args.workflow;
  }
  if (args.run) {
    config.run = args.run;
  }
  if (args.format) {
    config.format = args.format;
  }
  if (args["cookie-file"]) {
    config.cookieFile = args["cookie-file"];
  }
  if (args["fetch-logs"]) {
    config.fetchLogs = true;
  }
  if (args["failed-only"]) {
    config.failedOnly = true;
  }
  if (args["max-log-lines"]) {
    config.maxLogLines = Number(args["max-log-lines"]);
  }
  if (args["log-excerpt-lines"]) {
    config.logExcerptLines = Number(args["log-excerpt-lines"]);
  }
  if (args["log-context-lines"]) {
    config.logContextLines = Number(args["log-context-lines"]);
  }
  if (args["no-watch"]) {
    config.watch = false;
  }
  if (args.job) {
    config.job = String(args.job);
  }
  if (args["poll-interval"]) {
    config.pollInterval = Number(args["poll-interval"]);
  }
  config.configPath = configPath;
  config.cookieFile = path.resolve(repoRoot, config.cookieFile);
  return config;
}

function makeHeaders(cookie, extra = {}) {
  return {
    Accept: "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
    Cookie: cookie,
    "User-Agent":
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/136.0.0.0 Safari/537.36",
    ...extra,
  };
}

async function fetchText(urlOrPath, options = {}) {
  const url = urlOrPath.startsWith("http") ? urlOrPath : `${GITHUB_BASE}${urlOrPath}`;
  const response = await fetch(url, {
    method: "GET",
    redirect: "follow",
    headers: options.headers || {},
  });
  const text = await response.text();
  return {
    url: response.url,
    status: response.status,
    text,
  };
}

function ensureAuthenticated(response) {
  if (
    response.url.includes("/login") ||
    response.text.includes("Sign in to GitHub") ||
    response.text.includes("Create your account")
  ) {
    throw new Error("GitHub 登录态无效，请先重新执行 github-actions-cookie-login");
  }
}

function collectMatches(regex, text, mapper) {
  const results = [];
  let match;
  while ((match = regex.exec(text)) !== null) {
    results.push(mapper(match));
  }
  return results;
}

function normalizeSpace(value) {
  return stripTags(value).replace(/[ \t]+/g, " ").replace(/\n+/g, " ").trim();
}

function parseWorkflowRuns(html) {
  const pattern =
    /<div class="Box-row js-socket-channel js-updatable-content" id="check_suite_(\d+)"[\s\S]*?data-url="([^"]+)"[\s\S]*?<a href="([^"]*\/actions\/runs\/(\d+))"[^>]*aria-label="([^"]+)"/g;
  return collectMatches(pattern, html, (match) => {
    const aria = decodeHtml(match[5]);
    const status = aria.split(":")[0].trim();
    return {
      checkSuiteId: match[1],
      partialPath: decodeHtml(match[2]),
      runPath: decodeHtml(match[3]),
      runId: match[4],
      ariaLabel: aria,
      status,
    };
  });
}

function parseRunAnnotations(html, repo, runId) {
  const repoPrefix = repo.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const pattern = new RegExp(
    `href="/${repoPrefix}/actions/runs/${runId}/job/(\\d+)#step:(\\d+):(\\d+)"[\\s\\S]*?<div>([\\s\\S]*?)<\\/div>`,
    "g",
  );
  return collectMatches(pattern, html, (match) => ({
    jobId: match[1],
    stepNumber: Number(match[2]),
    column: Number(match[3]),
    message: normalizeSpace(match[4]),
  }));
}

function parseJobCards(html) {
  const pattern =
    /<streaming-graph-job[\s\S]*?(?:data-url="([^"]+)")?[\s\S]*?<a class="WorkflowJob-title[\s\S]*?href="([^"]*\/actions\/runs\/\d+\/job\/(\d+))"[\s\S]*?aria-label="([^"]+): "[\s\S]*?<span class="css-truncate css-truncate-overflow"[^>]*>\s*([\s\S]*?)\s*<\/span>[\s\S]*?<div class="flex-self-baseline text-small color-fg-muted flex-shrink-0 pl-1">\s*([\s\S]*?)\s*<\/div>/g;
  return collectMatches(pattern, html, (match) => ({
    streamPath: match[1] ? decodeHtml(match[1]) : null,
    jobPath: decodeHtml(match[2]),
    jobId: match[3],
    status: decodeHtml(match[4]).trim(),
    name: normalizeSpace(match[5]),
    duration: normalizeSpace(match[6]),
  }));
}

function parseMatrixExpansionUrls(html) {
  return collectMatches(/data-update-url="([^"]*expanded=true)"/g, html, (match) =>
    decodeHtml(match[1]),
  );
}

function parseWorkflowJobKeys(workflowFile) {
  const localPath = path.resolve(process.cwd(), `.github/workflows/${workflowFile}`);
  if (!fs.existsSync(localPath)) {
    return [];
  }
  const text = fs.readFileSync(localPath, "utf8");
  const jobsIdx = text.indexOf("\njobs:");
  if (jobsIdx < 0) {
    return [];
  }
  const afterJobs = text.slice(jobsIdx);
  const pattern = /\n  ([a-zA-Z_][\w-]*):/g;
  const keys = [];
  let match;
  while ((match = pattern.exec(afterJobs)) !== null) {
    if (!keys.includes(match[1])) {
      keys.push(match[1]);
    }
  }
  return keys;
}

function parseWorkflowMatrixOrder(workflowFile) {
  const localPath = path.resolve(process.cwd(), `.github/workflows/${workflowFile}`);
  if (!fs.existsSync(localPath)) {
    return new Map();
  }
  const text = fs.readFileSync(localPath, "utf8");
  const result = new Map();
  const jobPattern = /\n  ([a-zA-Z_][\w-]*):/g;
  let jobMatch;
  const jobPositions = [];
  while ((jobMatch = jobPattern.exec(text)) !== null) {
    jobPositions.push({ key: jobMatch[1], start: jobMatch.index });
  }
  for (let i = 0; i < jobPositions.length; i++) {
    const jobKey = jobPositions[i].key;
    const start = jobPositions[i].start;
    const end = i + 1 < jobPositions.length ? jobPositions[i + 1].start : text.length;
    const jobBlock = text.slice(start, end);
    const includeIdx = jobBlock.indexOf("include:");
    if (includeIdx < 0) {
      const osMatch = jobBlock.match(/os:\s*\[([^\]]+)\]/);
      if (osMatch) {
        const entries = osMatch[1].split(",").map((s) => s.trim().replace(/['"]/g, ""));
        result.set(jobKey, entries);
      }
      continue;
    }
    const afterInclude = jobBlock.slice(includeIdx);
    const entryBlocks = afterInclude.split(/\n\s+-\s+(?=\w+:)/).slice(1);
    const entries = [];
    for (const block of entryBlocks) {
      const targetMatch = block.match(/target:\s*(.+)/);
      const osMatch = block.match(/os:\s*(.+)/);
      const value = targetMatch ? targetMatch[1] : osMatch ? osMatch[1] : null;
      if (value) {
        entries.push(value.trim().replace(/['"]/g, ""));
      }
    }
    if (entries.length > 0) {
      result.set(jobKey, entries);
    }
  }
  return result;
}

function parseJobWorkflowKeys(html) {
  const keyToJobIds = new Map();
  const directPattern = /id="workflow-job-name-([^"]+)"[\s\S]*?href="[^"]*\/job\/(\d+)"/g;
  let match;
  while ((match = directPattern.exec(html)) !== null) {
    const key = match[1];
    if (!keyToJobIds.has(key)) {
      keyToJobIds.set(key, []);
    }
    keyToJobIds.get(key).push(match[2]);
  }
  return keyToJobIds;
}

function parseMatrixGroupKeys(html) {
  const urlToKey = new Map();
  const pattern = /data-update-url="([^"]*graph\/matrix\/([^?]+)\?[^"]*expanded=true)"/g;
  let match;
  while ((match = pattern.exec(html)) !== null) {
    const url = decodeHtml(match[1]);
    const token = match[2];
    try {
      const decoded = Buffer.from(token, "base64").toString("utf8");
      const keyMatch = decoded.match(/^\|-([\w-]+)-\|$/);
      if (keyMatch) {
        urlToKey.set(url, keyMatch[1]);
      }
    } catch (_) {}
  }
  return urlToKey;
}

async function parseAllJobsForRun(runHtml, cookie, workflowFile) {
  const jobs = new Map();
  for (const job of parseJobCards(runHtml)) {
    jobs.set(job.jobId, job);
  }

  const matrixUrls = parseMatrixExpansionUrls(runHtml);
  const matrixJobsByUrl = new Map();
  for (const matrixUrl of matrixUrls) {
    const response = await fetchText(matrixUrl, {
      headers: makeHeaders(cookie, {
        "X-Requested-With": "XMLHttpRequest",
      }),
    });
    if (response.status >= 400) {
      continue;
    }
    const expanded = parseJobCards(response.text);
    matrixJobsByUrl.set(matrixUrl, expanded);
    for (const job of expanded) {
      jobs.set(job.jobId, job);
    }
  }

  const yamlKeys = parseWorkflowJobKeys(workflowFile);
  if (yamlKeys.length === 0) {
    return Array.from(jobs.values());
  }

  const directKeyMap = parseJobWorkflowKeys(runHtml);
  const matrixKeyMap = parseMatrixGroupKeys(runHtml);
  const matrixOrder = parseWorkflowMatrixOrder(workflowFile);

  const keyToJobIds = new Map();
  for (const [key, jobIds] of directKeyMap) {
    keyToJobIds.set(key, jobIds);
  }
  for (const [url, key] of matrixKeyMap) {
    const expanded = matrixJobsByUrl.get(url);
    if (expanded) {
      const ids = keyToJobIds.get(key) || [];
      const matrixEntries = matrixOrder.get(key) || [];
      if (matrixEntries.length > 0) {
        const sorted = [...expanded].sort((a, b) => {
          const aIdx = matrixEntries.findIndex((e) => a.name.includes(e));
          const bIdx = matrixEntries.findIndex((e) => b.name.includes(e));
          const ai = aIdx >= 0 ? aIdx : Number.MAX_SAFE_INTEGER;
          const bi = bIdx >= 0 ? bIdx : Number.MAX_SAFE_INTEGER;
          return ai - bi;
        });
        for (const job of sorted) {
          ids.push(job.jobId);
        }
      } else {
        for (const job of expanded) {
          ids.push(job.jobId);
        }
      }
      keyToJobIds.set(key, ids);
    }
  }

  const ordered = [];
  const placed = new Set();
  for (const yamlKey of yamlKeys) {
    const jobIds = keyToJobIds.get(yamlKey);
    if (!jobIds) {
      continue;
    }
    for (const jobId of jobIds) {
      const job = jobs.get(jobId);
      if (job && !placed.has(jobId)) {
        ordered.push(job);
        placed.add(jobId);
      }
    }
  }
  for (const job of jobs.values()) {
    if (!placed.has(job.jobId)) {
      ordered.push(job);
    }
  }
  return ordered;
}

function extractAttrMap(tagSource) {
  const attrs = {};
  const attrPattern = /data-([a-z0-9-]+)="([^"]*)"/g;
  let match;
  while ((match = attrPattern.exec(tagSource)) !== null) {
    attrs[match[1]] = decodeHtml(match[2]);
  }
  return attrs;
}

function parseJobSteps(html) {
  const stepPattern = /<check-step\s+([\s\S]*?)>\s*<\/check-step>/g;
  return collectMatches(stepPattern, html, (match) => {
    const attrs = extractAttrMap(match[1]);
    return {
      name: attrs.name || "",
      number: attrs.number ? Number(attrs.number) : null,
      conclusion: attrs.conclusion || "in_progress",
      startedAt: attrs["started-at"] || null,
      completedAt: attrs["completed-at"] || null,
      logPath: attrs["log-url"] || null,
    };
  });
}

function parseJobAnnotations(html) {
  const pattern = /href="#annotation:(\d+):(\d+)"[\s\S]*?<div>([\s\S]*?)<\/div>/g;
  return collectMatches(pattern, html, (match) => ({
    stepNumber: Number(match[1]),
    column: Number(match[2]),
    message: normalizeSpace(match[3]),
  }));
}

function parseJobStatus(html) {
  const statusMatch = html.match(/<check-steps[\s\S]*?data-job-status="([^"]+)"/);
  return statusMatch ? decodeHtml(statusMatch[1]) : "unknown";
}

function findLikelyFailureText(logText) {
  const lines = String(logText || "").split(/\r?\n/);
  const interesting = [];
  for (const line of lines) {
    const lower = line.toLowerCase();
    if (
      lower.includes("error") ||
      lower.includes("failed") ||
      lower.includes("panic") ||
      lower.includes("exception") ||
      lower.includes("timed out")
    ) {
      interesting.push(line);
    }
  }
  return interesting.slice(-10);
}

function looksLikeFailureMessage(message) {
  const lower = String(message || "").toLowerCase();
  return (
    lower.includes("exit code") ||
    /\bfailed\b/.test(lower) ||
    /\bfailure\b/.test(lower) ||
    lower.includes("timed out") ||
    lower.includes("panic") ||
    lower.includes("exception") ||
    /\berror\b/.test(lower) ||
    lower.includes("##[error]")
  );
}

function looksLikeDiagnosticMessage(message) {
  const lower = String(message || "").toLowerCase();
  if (
    lower.includes("cache_on_failure") ||
    lower.includes("actions_allow_use_unsecure_node_version")
  ) {
    return false;
  }
  return (
    looksLikeFailureMessage(lower) ||
    /\bwarning\b/.test(lower) ||
    lower.includes("deprecated") ||
    lower.includes("cannot") ||
    lower.includes("unable") ||
    lower.includes("retry")
  );
}

function inferJobContext(jobName) {
  const lower = String(jobName || "").toLowerCase();
  let os = "unknown";
  if (lower.includes("windows")) {
    os = "windows";
  } else if (lower.includes("macos") || lower.includes("darwin")) {
    os = "macos";
  } else if (lower.includes("ubuntu") || lower.includes("linux")) {
    os = "linux";
  }

  let arch = "unknown";
  if (lower.includes("aarch64") || lower.includes("arm64")) {
    arch = "arm64";
  } else if (lower.includes("x86_64") || lower.includes("amd64")) {
    arch = "x64";
  }

  return { os, arch, runnerLabel: jobName };
}

function buildFailureSummary(failedSteps, annotations, relatedRunAnnotations) {
  const messages = [
    ...annotations.map((item) => item.message),
    ...relatedRunAnnotations.map((item) => item.message),
  ].filter(looksLikeFailureMessage);

  if (messages.length > 0) {
    return messages[0];
  }
  if (failedSteps.length > 0) {
    return `${failedSteps[0].name} failed`;
  }
  return null;
}

function trimAnsi(value) {
  return String(value || "").replace(/\u001b\[[0-9;]*m/g, "");
}

function uniqueNonEmpty(values) {
  return Array.from(
    new Set(
      values
        .map((value) => String(value || "").trim())
        .filter(Boolean),
    ),
  );
}

function isNoiseDiagnostic(line) {
  const lower = String(line || "").toLowerCase();
  return (
    lower.includes("process completed with exit code") ||
    lower.includes("test result: ok") ||
    lower.includes("test result: failed") ||
    lower.includes("warning:") ||
    lower.includes("##[warning]")
  );
}

function extractFailedTests(logText) {
  const lines = String(logText || "").split(/\r?\n/).map(trimAnsi);
  const failedTests = [];

  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index].trim();
    if (!/^failed tests:?$/i.test(line)) {
      continue;
    }
    for (let cursor = index + 1; cursor < lines.length; cursor += 1) {
      const candidate = lines[cursor].trim();
      if (!candidate) {
        break;
      }
      if (
        /^detail[: ]/i.test(candidate) ||
        /^error[: ]/i.test(candidate) ||
        /^fail(?:ed|ure)/i.test(candidate) ||
        /^thread ['"]/i.test(candidate) ||
        /^test result:/i.test(candidate)
      ) {
        break;
      }
      failedTests.push(candidate.replace(/^[\-\*\u2022]\s*/, ""));
    }
  }

  return uniqueNonEmpty(failedTests);
}

function extractRootCause(logText) {
  const lines = String(logText || "").split(/\r?\n/).map(trimAnsi);
  const candidates = [];

  for (const rawLine of lines) {
    const line = rawLine.trim();
    if (!line || isNoiseDiagnostic(line)) {
      continue;
    }

    if (
      /^detail[: ]/i.test(line) ||
      /^error[: ]/i.test(line) ||
      /^fail(?:ed|ure)[: ]/i.test(line) ||
      /^thread ['"].*panicked at/i.test(line) ||
      /\bassert(?:ion)?\b/i.test(line) ||
      /\bpanic\b/i.test(line) ||
      /\bexception\b/i.test(line) ||
      /\btimed out\b/i.test(line)
    ) {
      candidates.push(line);
      continue;
    }

    if (looksLikeFailureMessage(line) && !isNoiseDiagnostic(line)) {
      candidates.push(line);
    }
  }

  return uniqueNonEmpty(candidates).slice(0, 5);
}

function buildLogDiagnosis(logText) {
  return {
    failedTests: extractFailedTests(logText),
    rootCause: extractRootCause(logText),
  };
}

function extractLogExcerpt(logText, excerptLines) {
  const lines = String(logText || "").split(/\r?\n/);
  const matchIndexes = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (looksLikeDiagnosticMessage(lines[index])) {
      matchIndexes.push(index);
    }
  }

  if (matchIndexes.length === 0) {
    return lines.slice(-Math.max(1, excerptLines)).map(trimAnsi);
  }

  const start = Math.max(0, matchIndexes[0] - 5);
  const end = Math.min(lines.length, start + Math.max(1, excerptLines));
  return lines.slice(start, end).map(trimAnsi);
}

function extractLogContext(logText, contextLines) {
  const lines = String(logText || "").split(/\r?\n/);
  const matchIndexes = [];
  for (let index = 0; index < lines.length; index += 1) {
    if (looksLikeDiagnosticMessage(lines[index])) {
      matchIndexes.push(index);
    }
  }

  if (matchIndexes.length === 0) {
    return [];
  }

  const ranges = [];
  for (const matchIndex of matchIndexes) {
    const start = Math.max(0, matchIndex - Math.max(0, contextLines));
    const end = Math.min(lines.length - 1, matchIndex + Math.max(0, contextLines));
    const previous = ranges[ranges.length - 1];
    if (previous && start <= previous.end + 1) {
      previous.end = Math.max(previous.end, end);
    } else {
      ranges.push({ start, end });
    }
  }

  const output = [];
  for (let rangeIndex = 0; rangeIndex < ranges.length; rangeIndex += 1) {
    const range = ranges[rangeIndex];
    if (rangeIndex > 0) {
      output.push("... context gap ...");
    }
    for (let lineIndex = range.start; lineIndex <= range.end; lineIndex += 1) {
      output.push(trimAnsi(lines[lineIndex]));
    }
  }
  return output;
}

async function fetchStepLog(logPath, cookie, maxLogLines, logExcerptLines, logContextLines) {
  if (!logPath) {
    return null;
  }
  const response = await fetchText(logPath, {
    headers: makeHeaders(cookie, {
      Accept: "text/plain,*/*",
      "X-Requested-With": "XMLHttpRequest",
    }),
  });
  if (response.status >= 400) {
    return {
      available: false,
      status: response.status,
      preview: stripTags(response.text).slice(0, 400),
    };
  }
  if (response.url.includes("/login")) {
    return {
      available: false,
      status: response.status,
      preview: "login_required",
    };
  }
  const lines = response.text.split(/\r?\n/);
  const tail = lines.slice(-Math.max(1, maxLogLines));
  return {
    available: true,
    status: response.status,
    errorHints: findLikelyFailureText(response.text),
    diagnosis: buildLogDiagnosis(response.text),
    excerpt: extractLogExcerpt(response.text, logExcerptLines),
    context: extractLogContext(response.text, logContextLines),
    tail: tail.map(trimAnsi),
  };
}

async function loadWorkflowRun(config, cookie) {
  const workflowPath = `/${config.repo}/actions/workflows/${config.workflow}`;
  const workflowResponse = await fetchText(workflowPath, {
    headers: makeHeaders(cookie),
  });
  ensureAuthenticated(workflowResponse);

  const runs = parseWorkflowRuns(workflowResponse.text);
  if (runs.length === 0) {
    throw new Error(`未在 workflow 页面解析到 run: ${workflowPath}`);
  }

  let selectedRun = runs[0];
  if (config.run !== "latest") {
    const exact = runs.find((run) => run.runId === String(config.run));
    if (exact) {
      selectedRun = exact;
    } else {
      selectedRun = {
        runId: String(config.run),
        runPath: `/${config.repo}/actions/runs/${config.run}`,
        partialPath: null,
        checkSuiteId: null,
        ariaLabel: "user_selected",
        status: "user_selected",
      };
    }
  }

  const runResponse = await fetchText(selectedRun.runPath, {
    headers: makeHeaders(cookie),
  });
  ensureAuthenticated(runResponse);

  const jobs = await parseAllJobsForRun(runResponse.text, cookie, config.workflow);
  const runAnnotations = parseRunAnnotations(runResponse.text, config.repo, selectedRun.runId);

  return {
    workflowPath,
    runs,
    selectedRun,
    runHtml: runResponse.text,
    jobs,
    runAnnotations,
  };
}

async function inspectJob(job, runId, config, cookie, runAnnotations) {
  const response = await fetchText(job.jobPath, {
    headers: makeHeaders(cookie),
  });
  ensureAuthenticated(response);

  const steps = parseJobSteps(response.text);
  const jobStatus = parseJobStatus(response.text);
  const annotations = parseJobAnnotations(response.text);
  const relatedRunAnnotations = runAnnotations.filter((item) => item.jobId === job.jobId);
  const context = inferJobContext(job.name);

  const failedSteps = steps.filter((step) =>
    ["failure", "failed", "timed_out", "cancelled", "action_required"].includes(
      String(step.conclusion || "").toLowerCase(),
    ),
  );
  const interestingSteps = config.failedOnly
    ? failedSteps
    : steps.filter((step) => step.conclusion !== "success");

  const stepLogs = {};
  if (config.fetchLogs) {
    for (const step of interestingSteps) {
      if (!step.logPath) {
        continue;
      }
      stepLogs[step.number] = await fetchStepLog(
        step.logPath,
        cookie,
        config.maxLogLines,
        config.logExcerptLines,
        config.logContextLines,
      );
    }
  }

  return {
    runId,
    jobId: job.jobId,
    name: job.name,
    status: job.status,
    duration: job.duration,
    jobPath: job.jobPath,
    jobStatus,
    context,
    steps,
    failedSteps,
    annotations,
    relatedRunAnnotations,
    failureSummary: buildFailureSummary(failedSteps, annotations, relatedRunAnnotations),
    stepLogs,
  };
}

function formatText(result) {
  const lines = [];
  const diagnosticRunAnnotations = result.runAnnotations.filter((item) =>
    result.failedOnly ? looksLikeFailureMessage(item.message) : looksLikeDiagnosticMessage(item.message),
  );

  lines.push(`Repo: ${result.repo}`);
  lines.push(`Workflow: ${result.workflow}`);
  lines.push(`Run: ${result.run.runId}`);
  lines.push(`Run Status: ${result.run.status}`);
  lines.push(`Run Page: ${GITHUB_BASE}${result.run.runPath}`);
  lines.push("");

  const failedJobs = result.jobs.filter((job) => job.failedSteps.length > 0 || job.status.includes("failed"));
  const runningJobs = result.jobs.filter((job) => job.jobStatus === "in_progress" || job.status.includes("currently running"));
  lines.push(`Jobs: ${result.jobs.length} total, ${failedJobs.length} failed, ${runningJobs.length} running`);
  lines.push("");

  if (failedJobs.length > 0) {
    lines.push("Failure Digest:");
    for (const job of failedJobs) {
      lines.push(`- ${job.name}`);
      lines.push(`  os=${job.context.os} arch=${job.context.arch} status=${job.status}`);
      if (job.failureSummary) {
        lines.push(`  summary: ${job.failureSummary}`);
      }
      if (job.failedSteps[0]) {
        lines.push(
          `  failed-step: #${job.failedSteps[0].number} ${job.failedSteps[0].name}`,
        );
        if (job.failedSteps[0].logPath) {
          lines.push(`  log: ${GITHUB_BASE}${job.failedSteps[0].logPath}`);
        }
        const failedLog = job.stepLogs[job.failedSteps[0].number];
        if (failedLog && failedLog.available && failedLog.excerpt.length > 0) {
          if (failedLog.diagnosis.failedTests.length > 0) {
            lines.push(`  failed-tests: ${failedLog.diagnosis.failedTests.join(", ")}`);
          }
          if (failedLog.diagnosis.rootCause.length > 0) {
            lines.push(`  suspected-root-cause: ${failedLog.diagnosis.rootCause[0]}`);
          }
          lines.push("  excerpt:");
          for (const line of failedLog.excerpt) {
            lines.push(`    ${line}`);
          }
        }
        if (failedLog && failedLog.available && failedLog.context.length > 0) {
          lines.push("  error-context:");
          for (const line of failedLog.context) {
            lines.push(`    ${line}`);
          }
        }
      }
    }
    lines.push("");
  }

  if (diagnosticRunAnnotations.length > 0) {
    lines.push("Run Annotations:");
    for (const annotation of diagnosticRunAnnotations) {
      lines.push(`- job ${annotation.jobId} step ${annotation.stepNumber}: ${annotation.message}`);
    }
    lines.push("");
  }

  for (const job of result.jobs) {
    lines.push(`[${job.status}] ${job.name} (${job.duration})`);
    lines.push(`  ${GITHUB_BASE}${job.jobPath}`);
    lines.push(`  context: os=${job.context.os} arch=${job.context.arch}`);
    if (job.failureSummary) {
      lines.push(`  summary: ${job.failureSummary}`);
    }
    const steps = result.failedOnly ? job.failedSteps : job.steps;
    for (const step of steps) {
      if (result.failedOnly && job.failedSteps.length === 0) {
        continue;
      }
      lines.push(`  - step ${step.number} [${step.conclusion}] ${step.name}`);
      if (step.logPath) {
        lines.push(`    log: ${GITHUB_BASE}${step.logPath}`);
      }
      const log = job.stepLogs[step.number];
      if (log && log.available) {
        const hint = log.errorHints.join(" | ").trim();
        if (log.diagnosis.failedTests.length > 0) {
          lines.push(`    failed-tests: ${log.diagnosis.failedTests.join(", ")}`);
        }
        if (log.diagnosis.rootCause.length > 0) {
          lines.push("    suspected-root-cause:");
          for (const line of log.diagnosis.rootCause) {
            lines.push(`      ${line}`);
          }
        }
        if (hint) {
          lines.push(`    hints: ${hint}`);
        }
        if (log.excerpt.length > 0) {
          lines.push("    excerpt:");
          for (const line of log.excerpt) {
            lines.push(`      ${line}`);
          }
        }
        if (log.context.length > 0) {
          lines.push("    error-context:");
          for (const line of log.context) {
            lines.push(`      ${line}`);
          }
        }
      }
      if (log && !log.available) {
        lines.push(`    log-fetch: unavailable (${log.status}) ${log.preview}`);
      }
    }
    for (const annotation of job.annotations.filter((item) =>
      result.failedOnly ? looksLikeFailureMessage(item.message) : looksLikeDiagnosticMessage(item.message),
    )) {
      lines.push(`  - annotation step ${annotation.stepNumber}: ${annotation.message}`);
    }
    for (const annotation of job.relatedRunAnnotations.filter((item) =>
      result.failedOnly ? looksLikeFailureMessage(item.message) : looksLikeDiagnosticMessage(item.message),
    )) {
      lines.push(`  - run-annotation step ${annotation.stepNumber}: ${annotation.message}`);
    }
    lines.push("");
  }

  return lines.join("\n").trim();
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function isJobTerminal(status) {
  const lower = String(status || "").toLowerCase();
  return (
    lower.includes("completed") ||
    lower.includes("success") ||
    lower.includes("failed") ||
    lower.includes("failure") ||
    lower.includes("cancelled") ||
    lower.includes("skipped") ||
    lower.includes("timed_out")
  );
}

function isJobInProgress(status) {
  const lower = String(status || "").toLowerCase();
  return (
    lower.includes("in_progress") ||
    lower.includes("running") ||
    lower.includes("queued") ||
    lower.includes("waiting") ||
    lower.includes("pending") ||
    lower.includes("currently running")
  );
}

function formatTimestamp() {
  return new Date().toLocaleTimeString("en-US", { hour12: false });
}

function selectWatchTarget(jobs, config) {
  if (config.job) {
    const match = jobs.find((j) => j.jobId === config.job);
    if (!match) {
      const byName = jobs.find(
        (j) => j.name.toLowerCase().includes(config.job.toLowerCase()),
      );
      if (byName) {
        return [byName];
      }
      throw new Error(
        `未找到 job "${config.job}"，可用 jobs:\n${jobs.map((j) => `  ${j.jobId}: ${j.name} [${j.status}]`).join("\n")}`,
      );
    }
    return [match];
  }

  if (config.failedOnly) {
    const failed = jobs.filter(
      (j) =>
        String(j.status || "").toLowerCase().includes("failed") ||
        String(j.status || "").toLowerCase().includes("failure"),
    );
    if (failed.length > 0) {
      return failed;
    }
    console.log("⚠️  未找到失败的 job，将输出第一个运行中/等待中的 job");
  }

  const running = jobs.find((j) => isJobInProgress(j.status));
  if (running) {
    return [running];
  }

  const firstNonSuccess = jobs.find(
    (j) =>
      String(j.status || "").toLowerCase().includes("failed") ||
      String(j.status || "").toLowerCase().includes("failure"),
  );
  if (firstNonSuccess) {
    return [firstNonSuccess];
  }

  return jobs.length > 0 ? [jobs[0]] : [];
}

async function fetchAllStepLogs(steps, cookie) {
  const allLines = [];
  for (const step of steps) {
    if (!step.logPath) {
      continue;
    }
    const response = await fetchText(step.logPath, {
      headers: makeHeaders(cookie, {
        Accept: "text/plain,*/*",
        "X-Requested-With": "XMLHttpRequest",
      }),
    });
    if (response.status >= 400 || response.url.includes("/login")) {
      continue;
    }
    const lines = response.text.split(/\r?\n/);
    for (const line of lines) {
      allLines.push(trimAnsi(line));
    }
  }
  return allLines;
}

async function watchJobLogs(job, config, cookie) {
  let lastLineCount = 0;
  let consecutiveEmpty = 0;
  const maxConsecutiveEmpty = 3;

  console.log(`\n${"─".repeat(60)}`);
  console.log(`📋 Job: ${job.name}`);
  console.log(`🔗 ${GITHUB_BASE}${job.jobPath}`);
  console.log(`📊 Status: ${job.status}`);
  console.log(`${"─".repeat(60)}\n`);

  if (isJobTerminal(job.status) && !config.failedOnly) {
    const response = await fetchText(job.jobPath, {
      headers: makeHeaders(cookie),
    });
    ensureAuthenticated(response);
    const steps = parseJobSteps(response.text);
    const completedSteps = steps.filter((s) => s.conclusion && s.conclusion !== "in_progress");
    const allLines = await fetchAllStepLogs(completedSteps, cookie);
    if (allLines.length > 0) {
      for (const line of allLines) {
        console.log(line);
      }
    } else {
      console.log("(no log content available)");
    }
    console.log(`\n✅ Job "${job.name}" already ${job.status}`);
    return;
  }

  const seenLines = new Set();

  while (true) {
    try {
      const response = await fetchText(job.jobPath, {
        headers: makeHeaders(cookie),
      });
      ensureAuthenticated(response);

      const steps = parseJobSteps(response.text);
      const jobStatus = parseJobStatus(response.text);
      const activeSteps = steps.filter(
        (s) => s.conclusion === "in_progress" || (s.conclusion && s.conclusion !== "pending" && s.conclusion !== "queued"),
      );

      const allLines = await fetchAllStepLogs(activeSteps, cookie);

      let newLines = 0;
      for (let i = 0; i < allLines.length; i++) {
        const lineKey = `${i}:${allLines[i]}`;
        if (!seenLines.has(lineKey)) {
          seenLines.add(lineKey);
          console.log(allLines[i]);
          newLines++;
        }
      }

      if (newLines === 0) {
        consecutiveEmpty++;
      } else {
        consecutiveEmpty = 0;
      }

      lastLineCount = allLines.length;

      if (isJobTerminal(jobStatus)) {
        const finalSteps = parseJobSteps(response.text);
        const remainingSteps = finalSteps.filter(
          (s) => s.conclusion && s.conclusion !== "in_progress" && s.conclusion !== "pending" && s.conclusion !== "queued",
        );
        const finalLines = await fetchAllStepLogs(remainingSteps, cookie);
        for (let i = 0; i < finalLines.length; i++) {
          const lineKey = `${i}:${finalLines[i]}`;
          if (!seenLines.has(lineKey)) {
            seenLines.add(lineKey);
            console.log(finalLines[i]);
          }
        }

        const failedSteps = finalSteps.filter((s) =>
          ["failure", "failed", "timed_out", "cancelled"].includes(
            String(s.conclusion || "").toLowerCase(),
          ),
        );

        console.log(`\n${"─".repeat(60)}`);
        if (failedSteps.length > 0) {
          console.log(`❌ Job "${job.name}" finished with status: ${jobStatus}`);
          console.log(`   Failed steps:`);
          for (const step of failedSteps) {
            console.log(`     - #${step.number} ${step.name} [${step.conclusion}]`);
          }
        } else {
          console.log(`✅ Job "${job.name}" finished with status: ${jobStatus}`);
        }
        console.log(`${"─".repeat(60)}`);
        break;
      }

      if (consecutiveEmpty >= maxConsecutiveEmpty) {
        process.stderr.write(`[${formatTimestamp()}] ⏳ waiting for new output...\r`);
      }
    } catch (error) {
      console.error(`\n⚠️  [${formatTimestamp()}] poll error: ${error.message}`);
    }

    await sleep(config.pollInterval);
  }
}

async function runWatchMode(config) {
  const cookie = readCookieFile(config.cookieFile);
  const runData = await loadWorkflowRun(config, cookie);

  console.log(`🔍 Repo: ${config.repo}`);
  console.log(`🔄 Workflow: ${config.workflow}`);
  console.log(`🏃 Run: #${runData.selectedRun.runId} [${runData.selectedRun.status}]`);
  console.log(`🔗 ${GITHUB_BASE}${runData.selectedRun.runPath}`);

  const allJobs = runData.jobs;
  if (allJobs.length === 0) {
    console.log("⚠️  No jobs found in this run.");
    return;
  }

  console.log(`\n📦 Jobs (${allJobs.length} total):`);
  for (const job of allJobs) {
    const icon = isJobInProgress(job.status) ? "🔵" : isJobTerminal(job.status) && job.status.toLowerCase().includes("success") ? "🟢" : isJobTerminal(job.status) ? "🔴" : "⚪";
    console.log(`  ${icon} ${job.jobId}: ${job.name} [${job.status}] ${job.duration}`);
  }

  const targets = selectWatchTarget(allJobs, config);
  if (targets.length === 0) {
    console.log("⚠️  No matching jobs to watch.");
    return;
  }

  for (const target of targets) {
    await watchJobLogs(target, config, cookie);
  }
}

async function runClassicMode(config) {
  const cookie = readCookieFile(config.cookieFile);
  const runData = await loadWorkflowRun(config, cookie);

  const inspectedJobs = [];
  for (const job of runData.jobs) {
    const inspected = await inspectJob(
      job,
      runData.selectedRun.runId,
      config,
      cookie,
      runData.runAnnotations,
    );
    if (config.failedOnly) {
      const jobFailed =
        inspected.failedSteps.length > 0 ||
        String(inspected.status || "").toLowerCase().includes("failed") ||
        String(inspected.jobStatus || "").toLowerCase().includes("failed") ||
        inspected.annotations.some((item) => looksLikeFailureMessage(item.message)) ||
        inspected.relatedRunAnnotations.some((item) => looksLikeFailureMessage(item.message));
      if (!jobFailed) {
        continue;
      }
    }
    inspectedJobs.push(inspected);
  }

  const result = {
    repo: config.repo,
    workflow: config.workflow,
    configPath: config.configPath,
    cookieFile: config.cookieFile,
    run: runData.selectedRun,
    availableRuns: runData.runs,
    runAnnotations: runData.runAnnotations,
    jobs: inspectedJobs,
    failedOnly: config.failedOnly,
  };

  if (config.format === "json") {
    console.log(JSON.stringify(result, null, 2));
    return;
  }

  console.log(formatText(result));
}

async function main() {
  const args = parseArgs(process.argv);
  const config = mergeConfig(args);

  if (config.watch) {
    await runWatchMode(config);
  } else {
    await runClassicMode(config);
  }
}

main().catch((error) => {
  console.error(`❌ ${error.message}`);
  process.exit(1);
});
