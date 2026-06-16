async function assertVisible(ctx, params = {}) {
  const { uid, selector, timeout = 5000 } = params;

  const startTime = Date.now();

  while (Date.now() - startTime < timeout) {
    try {
      let element;
      if (uid) {
        element = await ctx.getElementByUid(uid);
      } else if (selector) {
        element = await ctx.page.$(selector);
      }

      if (element) {
        const isVisible = await element.isVisible();
        if (isVisible) {
          return { visible: true };
        }
      }
    } catch {}

    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  throw new Error(
    `Element ${uid || selector} is not visible within ${timeout}ms`,
  );
}

async function assertHidden(ctx, params = {}) {
  const { uid, selector, timeout = 5000 } = params;

  const startTime = Date.now();

  while (Date.now() - startTime < timeout) {
    try {
      let element;
      if (uid) {
        element = await ctx.getElementByUid(uid);
      } else if (selector) {
        element = await ctx.page.$(selector);
      }

      if (!element) {
        return { hidden: true };
      }

      const isVisible = await element.isVisible();
      if (!isVisible) {
        return { hidden: true };
      }
    } catch {
      return { hidden: true };
    }

    await new Promise((resolve) => setTimeout(resolve, 100));
  }

  throw new Error(
    `Element ${uid || selector} is still visible after ${timeout}ms`,
  );
}

async function assertText(ctx, params = {}) {
  const { uid, selector, text, contains = false } = params;

  if (text === undefined) {
    throw new Error("text is required");
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

  const actualText = await element.evaluate(
    (el) => el.textContent?.trim() || "",
  );

  if (contains) {
    if (actualText.includes(text)) {
      return { matched: true, actualText };
    }
    throw new Error(
      `Text "${text}" not found in element. Actual: "${actualText}"`,
    );
  }

  if (actualText === text) {
    return { matched: true, actualText };
  }

  throw new Error(
    `Text mismatch. Expected: "${text}", Actual: "${actualText}"`,
  );
}

async function assertValue(ctx, params = {}) {
  const { uid, selector, value } = params;

  if (value === undefined) {
    throw new Error("value is required");
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

  const actualValue = await element.evaluate((el) => el.value || "");

  if (actualValue === value) {
    return { matched: true, actualValue };
  }

  throw new Error(
    `Value mismatch. Expected: "${value}", Actual: "${actualValue}"`,
  );
}

async function assertChecked(ctx, params = {}) {
  const { uid, selector, checked = true } = params;

  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error("Either uid or selector is required");
  }

  const actualChecked = await element.evaluate((el) => el.checked || false);

  if (actualChecked === checked) {
    return { matched: true, checked: actualChecked };
  }

  throw new Error(
    `Checked state mismatch. Expected: ${checked}, Actual: ${actualChecked}`,
  );
}

async function assertDisabled(ctx, params = {}) {
  const { uid, selector, disabled = true } = params;

  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error("Either uid or selector is required");
  }

  const actualDisabled = await element.evaluate((el) => el.disabled || false);

  if (actualDisabled === disabled) {
    return { matched: true, disabled: actualDisabled };
  }

  throw new Error(
    `Disabled state mismatch. Expected: ${disabled}, Actual: ${actualDisabled}`,
  );
}

async function assertCount(ctx, params = {}) {
  const { selector, count } = params;

  if (!selector) {
    throw new Error("selector is required");
  }
  if (count === undefined) {
    throw new Error("count is required");
  }

  const elements = await ctx.page.$$(selector);
  const actualCount = elements.length;

  if (actualCount === count) {
    return { matched: true, count: actualCount };
  }

  throw new Error(`Count mismatch. Expected: ${count}, Actual: ${actualCount}`);
}

async function assertPageTitle(ctx, params = {}) {
  const { title, contains = false } = params;

  if (!title) {
    throw new Error("title is required");
  }

  const actualTitle = await ctx.page.title();

  if (contains) {
    if (actualTitle.includes(title)) {
      return { matched: true, actualTitle };
    }
    throw new Error(`Title "${title}" not found. Actual: "${actualTitle}"`);
  }

  if (actualTitle === title) {
    return { matched: true, actualTitle };
  }

  throw new Error(
    `Title mismatch. Expected: "${title}", Actual: "${actualTitle}"`,
  );
}

async function assertUrl(ctx, params = {}) {
  const { url, pattern, partial = false } = params;

  const actualUrl = ctx.page.url();

  if (pattern) {
    const regex = new RegExp(pattern);
    if (regex.test(actualUrl)) {
      return { matched: true, actualUrl };
    }
    throw new Error(
      `URL does not match pattern "${pattern}". Actual: "${actualUrl}"`,
    );
  }

  if (url) {
    if (partial ? actualUrl.includes(url) : actualUrl === url) {
      return { matched: true, actualUrl };
    }
    throw new Error(`URL mismatch. Expected: "${url}", Actual: "${actualUrl}"`);
  }

  throw new Error("Either url or pattern is required");
}

async function assertNoErrors(ctx, params = {}) {
  const { ignorePatterns = [] } = params;

  const errors = ctx.getConsoleCollector().getErrors();

  const filteredErrors = errors.filter((error) => {
    const text = error.text || "";
    return !ignorePatterns.some((pattern) => {
      const regex = typeof pattern === "string" ? new RegExp(pattern) : pattern;
      return regex.test(text);
    });
  });

  if (filteredErrors.length === 0) {
    return { noErrors: true, errorCount: 0 };
  }

  throw new Error(
    `Found ${filteredErrors.length} console error(s): ${filteredErrors.map((e) => e.text).join("; ")}`,
  );
}

module.exports = {
  assertVisible,
  assertHidden,
  assertText,
  assertValue,
  assertChecked,
  assertDisabled,
  assertCount,
  assertPageTitle,
  assertUrl,
  assertNoErrors,
};
