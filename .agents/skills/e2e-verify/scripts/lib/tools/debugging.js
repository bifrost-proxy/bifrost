async function getConsoleLogs(ctx, params = {}) {
  const { level, limit = 100 } = params;

  let messages = ctx.getConsoleCollector().getMessages();

  if (level) {
    const levels = Array.isArray(level) ? level : [level];
    messages = messages.filter((m) => levels.includes(m.type));
  }

  const limitedMessages = messages.slice(-limit);

  return {
    total: messages.length,
    messages: limitedMessages.map((m) => ({
      id: m.id,
      type: m.type,
      text: m.text,
      location: m.location,
      timestamp: m.timestamp,
    })),
  };
}

async function getConsoleErrors(ctx, params = {}) {
  const { limit = 100 } = params;

  const errors = ctx.getConsoleCollector().getErrors();
  const limitedErrors = errors.slice(-limit);

  return {
    total: errors.length,
    errors: limitedErrors.map((e) => ({
      id: e.id,
      type: e.type,
      text: e.text,
      stack: e.stack,
      location: e.location,
      timestamp: e.timestamp,
    })),
  };
}

async function getConsoleWarnings(ctx, params = {}) {
  const { limit = 100 } = params;

  const warnings = ctx.getConsoleCollector().getWarnings();
  const limitedWarnings = warnings.slice(-limit);

  return {
    total: warnings.length,
    warnings: limitedWarnings.map((w) => ({
      id: w.id,
      text: w.text,
      location: w.location,
      timestamp: w.timestamp,
    })),
  };
}

async function clearConsoleLogs(ctx, params = {}) {
  const consoleCollector = ctx.getConsoleCollector();
  const previousCount = consoleCollector.getMessages().length;
  consoleCollector.clear();

  return { cleared: previousCount };
}

async function evaluateScript(ctx, params = {}) {
  const { script, expression } = params;
  const code = script || expression;

  if (!code) {
    throw new Error("script or expression is required");
  }

  const result = await ctx.page.evaluate(code);

  return { result };
}

async function evaluateOnElement(ctx, params = {}) {
  const { uid, selector, script, expression } = params;
  const code = script || expression;

  if (!code) {
    throw new Error("script or expression is required");
  }

  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error("Either uid or selector is required");
  }

  const result = await element.evaluate(
    (el, expr) => new Function("element", `return ${expr}`)(el),
    code,
  );

  return { result };
}

async function getPageMetrics(ctx, params = {}) {
  const metrics = await ctx.page.metrics();

  return {
    timestamp: metrics.Timestamp,
    documents: metrics.Documents,
    frames: metrics.Frames,
    jsEventListeners: metrics.JSEventListeners,
    nodes: metrics.Nodes,
    layoutCount: metrics.LayoutCount,
    recalcStyleCount: metrics.RecalcStyleCount,
    layoutDuration: metrics.LayoutDuration,
    recalcStyleDuration: metrics.RecalcStyleDuration,
    scriptDuration: metrics.ScriptDuration,
    taskDuration: metrics.TaskDuration,
    jsHeapUsedSize: metrics.JSHeapUsedSize,
    jsHeapTotalSize: metrics.JSHeapTotalSize,
  };
}

async function getPerformanceTiming(ctx, params = {}) {
  const timing = await ctx.page.evaluate(() => {
    const perf = window.performance;
    const timing = perf.timing;
    const navigation = perf.getEntriesByType("navigation")[0] || {};

    return {
      loadEventEnd: timing.loadEventEnd - timing.navigationStart,
      domContentLoaded:
        timing.domContentLoadedEventEnd - timing.navigationStart,
      firstPaint: navigation.domContentLoadedEventEnd || null,
      domInteractive: timing.domInteractive - timing.navigationStart,
      responseEnd: timing.responseEnd - timing.navigationStart,
      connectEnd: timing.connectEnd - timing.navigationStart,
    };
  });

  return timing;
}

async function getCoverage(ctx, params = {}) {
  const { type = "js" } = params;

  if (type === "js") {
    const jsCoverage = await ctx.page.coverage.stopJSCoverage();

    const coverage = jsCoverage.map((entry) => {
      const total = entry.text.length;
      const used = entry.ranges.reduce(
        (acc, range) => acc + (range.end - range.start),
        0,
      );
      return {
        url: entry.url,
        total,
        used,
        percentage: total ? Math.round((used / total) * 100) : 0,
      };
    });

    return { type: "js", coverage };
  }

  if (type === "css") {
    const cssCoverage = await ctx.page.coverage.stopCSSCoverage();

    const coverage = cssCoverage.map((entry) => {
      const total = entry.text.length;
      const used = entry.ranges.reduce(
        (acc, range) => acc + (range.end - range.start),
        0,
      );
      return {
        url: entry.url,
        total,
        used,
        percentage: total ? Math.round((used / total) * 100) : 0,
      };
    });

    return { type: "css", coverage };
  }

  throw new Error(`Unknown coverage type: ${type}. Use 'js' or 'css'`);
}

async function startCoverage(ctx, params = {}) {
  const { type = "both" } = params;

  if (type === "js" || type === "both") {
    await ctx.page.coverage.startJSCoverage();
  }

  if (type === "css" || type === "both") {
    await ctx.page.coverage.startCSSCoverage();
  }

  return { started: type };
}

async function getStatus(ctx, params = {}) {
  const url = ctx.page.url();
  const title = await ctx.page.title();
  const viewport = ctx.page.viewport();

  return {
    url,
    title,
    viewport,
    networkRequests: ctx.getNetworkCollector().getRequests().length,
    consoleMessages: ctx.getConsoleCollector().getMessages().length,
    consoleErrors: ctx.getConsoleCollector().getErrors().length,
    variables: Object.keys(ctx.getVariables()),
  };
}

async function waitForDebugger(ctx, params = {}) {
  const { timeout = 30000 } = params;

  const client = await ctx.page.target().createCDPSession();
  await client.send("Debugger.enable");
  await client.send("Debugger.pause");

  return { paused: true, timeout };
}

async function resumeDebugger(ctx, params = {}) {
  const client = await ctx.page.target().createCDPSession();
  await client.send("Debugger.resume");

  return { resumed: true };
}

module.exports = {
  getConsoleLogs,
  getConsoleErrors,
  getConsoleWarnings,
  clearConsoleLogs,
  evaluateScript,
  evaluateOnElement,
  getPageMetrics,
  getPerformanceTiming,
  getCoverage,
  startCoverage,
  getStatus,
  waitForDebugger,
  resumeDebugger,
};
