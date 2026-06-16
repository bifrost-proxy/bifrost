async function navigate(ctx, params = {}) {
  const { url, waitUntil = 'networkidle0' } = params;
  
  if (!url) {
    throw new Error('url is required');
  }
  
  await ctx.page.goto(url, { waitUntil });
  
  return {
    navigated: true,
    url: ctx.page.url()
  };
}

async function goBack(ctx, params = {}) {
  const { waitUntil = 'networkidle0' } = params;
  
  await ctx.page.goBack({ waitUntil });
  
  return {
    navigated: true,
    url: ctx.page.url()
  };
}

async function goForward(ctx, params = {}) {
  const { waitUntil = 'networkidle0' } = params;
  
  await ctx.page.goForward({ waitUntil });
  
  return {
    navigated: true,
    url: ctx.page.url()
  };
}

async function reload(ctx, params = {}) {
  const { waitUntil = 'networkidle0' } = params;
  
  await ctx.page.reload({ waitUntil });
  
  return {
    reloaded: true,
    url: ctx.page.url()
  };
}

async function waitForNavigation(ctx, params = {}) {
  const { waitUntil = 'networkidle0', timeout = 30000 } = params;
  
  await ctx.page.waitForNavigation({ waitUntil, timeout });
  
  return {
    completed: true,
    url: ctx.page.url()
  };
}

async function waitForLoad(ctx, params = {}) {
  const { state = 'networkidle0', timeout = 30000 } = params;
  
  if (state === 'networkidle0' || state === 'networkidle2') {
    await ctx.page.waitForNetworkIdle({ timeout });
  } else if (state === 'domcontentloaded') {
    await ctx.page.waitForFunction(
      () => document.readyState === 'interactive' || document.readyState === 'complete',
      { timeout }
    );
  } else if (state === 'load') {
    await ctx.page.waitForFunction(
      () => document.readyState === 'complete',
      { timeout }
    );
  }
  
  return {
    loaded: true,
    state,
    url: ctx.page.url()
  };
}

async function getPageInfo(ctx, params = {}) {
  const title = await ctx.page.title();
  const url = ctx.page.url();
  const viewport = ctx.page.viewport();
  const readyState = await ctx.page.evaluate(() => document.readyState);
  
  return {
    title,
    url,
    viewport,
    readyState
  };
}

async function waitForUrl(ctx, params = {}) {
  const { url, pattern, timeout = 30000, partial = false } = params;
  
  const startTime = Date.now();
  const targetPattern = pattern ? new RegExp(pattern) : null;
  
  while (Date.now() - startTime < timeout) {
    const currentUrl = ctx.page.url();
    
    if (targetPattern) {
      if (targetPattern.test(currentUrl)) {
        return { matched: true, url: currentUrl };
      }
    } else if (url) {
      if (partial ? currentUrl.includes(url) : currentUrl === url) {
        return { matched: true, url: currentUrl };
      }
    }
    
    await new Promise(resolve => setTimeout(resolve, 100));
  }
  
  throw new Error(`URL did not match within ${timeout}ms. Current: ${ctx.page.url()}`);
}

async function setExtraHTTPHeaders(ctx, params = {}) {
  const { headers } = params;
  
  if (!headers || typeof headers !== 'object') {
    throw new Error('headers object is required');
  }
  
  await ctx.page.setExtraHTTPHeaders(headers);
  
  return {
    set: true,
    headers: Object.keys(headers)
  };
}

async function setUserAgent(ctx, params = {}) {
  const { userAgent } = params;
  
  if (!userAgent) {
    throw new Error('userAgent is required');
  }
  
  await ctx.page.setUserAgent(userAgent);
  
  return {
    set: true,
    userAgent
  };
}

module.exports = {
  navigate,
  goBack,
  goForward,
  reload,
  waitForNavigation,
  waitForLoad,
  getPageInfo,
  waitForUrl,
  setExtraHTTPHeaders,
  setUserAgent
};
