async function getNetworkRequests(ctx, params = {}) {
  const {
    filter,
    status,
    method,
    resourceType,
    urlPattern,
    limit = 100,
  } = params;

  const networkCollector = ctx.getNetworkCollector();
  let requests = networkCollector.getRequests();

  if (urlPattern) {
    const pattern =
      typeof urlPattern === "string" ? new RegExp(urlPattern) : urlPattern;
    requests = requests.filter((r) => pattern.test(r.url));
  }

  if (status) {
    requests = requests.filter((r) => r.status === status);
  }

  if (method) {
    requests = requests.filter((r) => r.method === method);
  }

  if (resourceType) {
    requests = requests.filter((r) => r.resourceType === resourceType);
  }

  if (filter && typeof filter === "function") {
    requests = requests.filter(filter);
  }

  return {
    total: requests.length,
    requests: requests.slice(0, limit).map((r) => ({
      id: r.id,
      url: r.url,
      method: r.method,
      resourceType: r.resourceType,
      status: r.status,
      duration: r.duration,
    })),
  };
}

async function getRequestContent(ctx, params = {}) {
  const { id } = params;

  if (!id) {
    throw new Error("id is required");
  }

  const networkCollector = ctx.getNetworkCollector();
  const request = networkCollector.getRequestById(id);

  if (!request) {
    throw new Error(`Request with id ${id} not found`);
  }

  return {
    id: request.id,
    url: request.url,
    method: request.method,
    resourceType: request.resourceType,
    status: request.status,
    duration: request.duration,
    headers: request.headers,
    postData: request.postData,
    response: request.response,
    timestamp: request.timestamp,
  };
}

async function getFailedRequests(ctx, params = {}) {
  const { limit = 100 } = params;

  const networkCollector = ctx.getNetworkCollector();
  const failedRequests = networkCollector.getFailedRequests();

  return {
    total: failedRequests.length,
    requests: failedRequests.slice(0, limit).map((r) => ({
      id: r.id,
      url: r.url,
      method: r.method,
      status: r.status,
      error: r.error,
    })),
  };
}

async function waitForRequest(ctx, params = {}) {
  const { url, urlPattern, timeout = 30000 } = params;

  const pattern = urlPattern || url;
  if (!pattern) {
    throw new Error("url or urlPattern is required");
  }

  const networkCollector = ctx.getNetworkCollector();
  const request = await networkCollector.waitForRequest(pattern, timeout);

  return {
    url: request.url(),
    method: request.method(),
    resourceType: request.resourceType(),
  };
}

async function waitForResponse(ctx, params = {}) {
  const { url, urlPattern, timeout = 30000 } = params;

  const pattern = urlPattern || url;
  if (!pattern) {
    throw new Error("url or urlPattern is required");
  }

  const networkCollector = ctx.getNetworkCollector();
  const response = await networkCollector.waitForResponse(pattern, timeout);

  return {
    url: response.url(),
    status: response.status(),
    statusText: response.statusText(),
  };
}

async function clearNetworkRequests(ctx, params = {}) {
  const networkCollector = ctx.getNetworkCollector();
  const previousCount = networkCollector.getRequests().length;
  networkCollector.clear();

  return { cleared: previousCount };
}

async function setRequestInterception(ctx, params = {}) {
  const { enabled } = params;

  await ctx.page.setRequestInterception(enabled);

  return { enabled };
}

async function mockRequest(ctx, params = {}) {
  const { url, urlPattern, response, once = false } = params;

  const pattern = urlPattern || url;
  if (!pattern) {
    throw new Error("url or urlPattern is required");
  }
  if (!response) {
    throw new Error("response is required");
  }

  await ctx.page.setRequestInterception(true);

  const regex = typeof pattern === "string" ? new RegExp(pattern) : pattern;
  let intercepted = false;

  const handler = (request) => {
    if (regex.test(request.url())) {
      if (once && intercepted) {
        request.continue();
        return;
      }

      intercepted = true;

      request.respond({
        status: response.status || 200,
        contentType: response.contentType || "application/json",
        headers: response.headers || {},
        body:
          typeof response.body === "string"
            ? response.body
            : JSON.stringify(response.body),
      });
    } else {
      request.continue();
    }
  };

  ctx.page.on("request", handler);
  ctx.setVariable(`_mock_handler_${pattern}`, handler);

  return { mocked: true, pattern: pattern.toString() };
}

async function blockRequests(ctx, params = {}) {
  const { patterns, resourceTypes } = params;

  await ctx.page.setRequestInterception(true);

  const regexPatterns = (patterns || []).map((p) =>
    typeof p === "string" ? new RegExp(p) : p,
  );

  const handler = (request) => {
    const shouldBlock =
      (regexPatterns.length > 0 &&
        regexPatterns.some((p) => p.test(request.url()))) ||
      (resourceTypes && resourceTypes.includes(request.resourceType()));

    if (shouldBlock) {
      request.abort();
    } else {
      request.continue();
    }
  };

  ctx.page.on("request", handler);
  ctx.setVariable("_block_handler", handler);

  return { blocked: true };
}

async function getNetworkStats(ctx, params = {}) {
  const networkCollector = ctx.getNetworkCollector();
  const requests = networkCollector.getRequests();

  const stats = {
    total: requests.length,
    successful: requests.filter((r) => r.status && r.status < 400).length,
    failed: requests.filter((r) => r.error || (r.status && r.status >= 400))
      .length,
    pending: requests.filter((r) => r.status === null && !r.error).length,
    byResourceType: {},
    byMethod: {},
    totalDuration: 0,
    averageDuration: 0,
  };

  for (const request of requests) {
    stats.byResourceType[request.resourceType] =
      (stats.byResourceType[request.resourceType] || 0) + 1;
    stats.byMethod[request.method] = (stats.byMethod[request.method] || 0) + 1;

    if (request.duration) {
      stats.totalDuration += request.duration;
    }
  }

  if (stats.total > 0) {
    stats.averageDuration = Math.round(stats.totalDuration / stats.total);
  }

  return stats;
}

module.exports = {
  getNetworkRequests,
  getRequestContent,
  getFailedRequests,
  waitForRequest,
  waitForResponse,
  clearNetworkRequests,
  setRequestInterception,
  mockRequest,
  blockRequests,
  getNetworkStats,
};
