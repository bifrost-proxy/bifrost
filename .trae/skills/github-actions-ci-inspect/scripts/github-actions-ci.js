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

async function parseAllJobsForRun(runHtml, cookie) {
  const jobs = new Map();
  for (const job of parseJobCards(runHtml)) {
    jobs.set(job.jobId, job);
  }

  const matrixUrls = parseMatrixExpansionUrls(runHtml);
  for (const matrixUrl of matrixUrls) {
    const response = await fetchText(matrixUrl, {
      headers: makeHeaders(cookie, {
        "X-Requested-With": "XMLHttpRequest",
      }),
    });
    if (response.status >= 400) {
      continue;
    }
    for (const job of parseJobCards(response.text)) {
      jobs.set(job.jobId, job);
    }
  }

  return Array.from(jobs.values());
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

  const jobs = await parseAllJobsForRun(runResponse.text, cookie);
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

async function main() {
  const args = parseArgs(process.argv);
  const config = mergeConfig(args);
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
  };

  if (config.format === "json") {
    console.log(JSON.stringify(result, null, 2));
    return;
  }

  console.log(formatText(result));
}

main().catch((error) => {
  console.error(`❌ ${error.message}`);
  process.exit(1);
});
