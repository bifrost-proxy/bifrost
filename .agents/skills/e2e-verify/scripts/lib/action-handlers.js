const fs = require("fs");
const path = require("path");
const { getToolByName } = require("./tools");

const LOGS_DIR = path.join(__dirname, "../../logs");
const SNAPSHOT_MAX_LENGTH = 5000;

const actionHandlers = {
  async goto(ctx, params, executor) {
    const { url } = params;
    const page = ctx.getPage();
    const baseUrl = executor.config.baseUrl || "";
    const fullUrl = url.startsWith("http") ? url : `${baseUrl}${url}`;

    await page.goto(fullUrl, {
      waitUntil: "domcontentloaded",
      timeout: params.timeout || 30000,
    });

    await new Promise((r) => setTimeout(r, 1000));
    return { navigated: fullUrl };
  },

  async waitForLogin(ctx, params, executor) {
    const page = ctx.getPage();
    const timeout = params.timeout || executor.config.loginTimeout || 120000;
    const startTime = Date.now();
    const checkInterval = 2000;

    executor.log("info", "等待登录完成...");

    while (Date.now() - startTime < timeout) {
      const url = page.url();

      if (!url.includes("/login") && !url.includes("/sso")) {
        try {
          const hasContent = await page.evaluate(() => {
            const body = document.body;
            if (!body) return false;
            const text = body.innerText || "";
            return (
              text.length > 100 &&
              !text.includes("401") &&
              !text.includes("Unauthorized")
            );
          });

          if (hasContent) {
            const waitedTime = Math.round((Date.now() - startTime) / 1000);
            executor.log("info", `登录检测成功 (${waitedTime}s)`);
            await new Promise((r) => setTimeout(r, 2000));
            return { success: true, waitedTime };
          }
        } catch {}
      }

      await new Promise((r) => setTimeout(r, checkInterval));
    }

    throw new Error("登录超时，请在浏览器中完成登录");
  },

  async takeSnapshot(ctx, params, executor) {
    const snapshot = await ctx.createSnapshot();
    const formatted = snapshot.formatSnapshot();

    if (params.name) {
      executor.log("snapshot", `[${params.name}] 快照已创建`);
    }

    const maxLen = params.maxLength || SNAPSHOT_MAX_LENGTH;
    const isTruncated = formatted.length > maxLen;
    let snapshotFile = null;

    if (isTruncated) {
      if (!fs.existsSync(LOGS_DIR)) {
        fs.mkdirSync(LOGS_DIR, { recursive: true });
      }
      const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
      const filename = `snapshot-${timestamp}.txt`;
      snapshotFile = path.join(LOGS_DIR, filename);
      fs.writeFileSync(snapshotFile, formatted, "utf-8");
    }

    if (params.print !== false) {
      if (isTruncated) {
        const preview = formatted.substring(0, maxLen);
        const remaining = formatted.length - maxLen;
        console.log("\n" + preview);
        console.log(`\n...(已截断，还有 ${remaining} 字符未显示)\n`);
        console.log(`完整快照已保存至: ${snapshotFile}\n`);
      } else {
        console.log("\n" + formatted + "\n");
      }
    }

    if (isTruncated) {
      return {
        snapshot: formatted.substring(0, maxLen),
        snapshotFile,
        message: `快照内容较长(${formatted.length}字符)，完整内容已保存至: ${snapshotFile}`,
      };
    }
    return { snapshot: formatted };
  },

  async findElement(ctx, params, executor) {
    const { role, name, query, variable } = params;
    const snapshot = await ctx.getOrCreateSnapshot();

    let results = [];
    if (role) {
      results = snapshot.findNodesByRole(role);
      if (name) {
        results = results.filter((n) => n.name && n.name.includes(name));
      }
    } else if (name) {
      results = snapshot.findNodesByName(name);
    } else if (query) {
      results = snapshot.findNodes(query);
    }

    if (results.length === 0) {
      throw new Error(`未找到元素: role=${role}, name=${name}, query=${query}`);
    }

    const element = results[0];
    const uid = element.id;

    if (variable) {
      executor.variables.set(variable, uid);
    }

    return { uid, role: element.role, name: element.name };
  },

  async click(ctx, params, executor) {
    let { uid, role, name, selector } = params;

    if (!uid && (role || name)) {
      const result = await actionHandlers.findElement(
        ctx,
        { role, name },
        executor,
      );
      uid = result.uid;
    }

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.click();
      executor.log("action", `点击元素 [${uid}]`);
      return { clicked: uid };
    }

    if (selector) {
      const page = ctx.getPage();
      await page.click(selector);
      executor.log("action", `点击选择器 ${selector}`);
      return { clicked: selector };
    }

    throw new Error("click 需要 uid, role+name, 或 selector");
  },

  async fill(ctx, params, executor) {
    let { uid, role, name, selector, value } = params;

    if (!uid && (role || name)) {
      const result = await actionHandlers.findElement(
        ctx,
        { role, name },
        executor,
      );
      uid = result.uid;
    }

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.click({ clickCount: 3 });
      await element.type(value);
      executor.log("action", `填写元素 [${uid}]: ${value}`);
      return { filled: uid, value };
    }

    if (selector) {
      const page = ctx.getPage();
      await page.click(selector, { clickCount: 3 });
      await page.type(selector, value);
      executor.log("action", `填写选择器 ${selector}: ${value}`);
      return { filled: selector, value };
    }

    throw new Error("fill 需要 uid, role+name, 或 selector");
  },

  async select(ctx, params, executor) {
    const { uid, selector, value } = params;
    const page = ctx.getPage();

    if (selector) {
      await page.select(selector, value);
      executor.log("action", `选择 ${selector}: ${value}`);
      return { selected: value };
    }

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.select(value);
      executor.log("action", `选择 [${uid}]: ${value}`);
      return { selected: value };
    }

    throw new Error("select 需要 uid 或 selector");
  },

  async wait(ctx, params, executor) {
    const ms = params.ms || params.timeout || 1000;
    await new Promise((r) => setTimeout(r, ms));
    return { waited: ms };
  },

  async waitForSelector(ctx, params, executor) {
    const { selector, timeout = 30000, visible = true } = params;
    const page = ctx.getPage();

    await page.waitForSelector(selector, { timeout, visible });
    executor.log("info", `选择器就绪: ${selector}`);
    return { found: selector };
  },

  async waitForNavigation(ctx, params, executor) {
    const { timeout = 30000, waitUntil = "networkidle0" } = params;
    const page = ctx.getPage();

    await page.waitForNavigation({ timeout, waitUntil });
    executor.log("info", `导航完成: ${page.url()}`);
    return { url: page.url() };
  },

  async screenshot(ctx, params, executor) {
    const tool = getToolByName("screenshot");
    const result = await tool.handler(ctx, {
      name: params.name || `step-${Date.now()}`,
    });
    executor.log("info", `截图已保存: ${result.path}`);
    return result;
  },

  async evaluate(ctx, params, executor) {
    const { code, variable } = params;
    const page = ctx.getPage();

    const result = await page.evaluate((js) => {
      try {
        return eval(js);
      } catch (e) {
        return { error: e.message };
      }
    }, code);

    if (variable) {
      const value =
        typeof result === "object" ? JSON.stringify(result) : String(result);
      executor.variables.set(variable, value);
    }

    return { result };
  },

  async assert(ctx, params, executor) {
    const { condition, message } = params;
    const page = ctx.getPage();

    let result;
    if (condition.includes("document") || condition.includes("window")) {
      result = await page.evaluate((cond) => {
        try {
          return eval(cond);
        } catch (e) {
          return false;
        }
      }, condition);
    } else {
      result = executor.variables.evaluateCondition(condition);
    }

    if (!result) {
      throw new Error(`断言失败: ${message || condition}`);
    }

    executor.log("assert", `✓ ${message || condition}`);
    return { passed: true };
  },

  async log(ctx, params, executor) {
    const { message, level = "info" } = params;
    executor.log(level, message);
    return { logged: message };
  },

  async if(ctx, params, executor) {
    const { condition, then: thenSteps, else: elseSteps } = params;
    const page = ctx.getPage();

    let result;
    if (condition.includes("document") || condition.includes("window")) {
      result = await page.evaluate((cond) => {
        try {
          return eval(cond);
        } catch {
          return false;
        }
      }, condition);
    } else {
      result = executor.variables.evaluateCondition(condition);
    }

    const stepsToRun = result ? thenSteps : elseSteps;
    if (stepsToRun && stepsToRun.length > 0) {
      for (const step of stepsToRun) {
        await executor.executeStep(step);
      }
    }

    return { condition: result, branch: result ? "then" : "else" };
  },

  async storeVariable(ctx, params, executor) {
    const { name, value } = params;
    executor.variables.set(name, value);
    return { stored: { name, value } };
  },

  async hover(ctx, params, executor) {
    let { uid, selector } = params;

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.hover();
      executor.log("action", `悬停元素 [${uid}]`);
      return { hovered: uid };
    }

    if (selector) {
      const page = ctx.getPage();
      await page.hover(selector);
      executor.log("action", `悬停选择器 ${selector}`);
      return { hovered: selector };
    }

    throw new Error("hover 需要 uid 或 selector");
  },

  async press(ctx, params, executor) {
    const { key } = params;
    const page = ctx.getPage();
    await page.keyboard.press(key);
    executor.log("action", `按键: ${key}`);
    return { pressed: key };
  },

  async type(ctx, params, executor) {
    const { text, delay = 0 } = params;
    const page = ctx.getPage();
    await page.keyboard.type(text, { delay });
    executor.log("action", `输入文本: ${text.substring(0, 50)}`);
    return { typed: text };
  },

  async scrollTo(ctx, params, executor) {
    const { x = 0, y = 0, uid, selector } = params;
    const page = ctx.getPage();

    if (uid || selector) {
      const target = uid ? `::-p-aria([name="${uid}"])` : selector;
      await page.evaluate((sel) => {
        const el = document.querySelector(sel);
        if (el) el.scrollIntoView({ behavior: "smooth", block: "center" });
      }, target);
    } else {
      await page.evaluate((coords) => window.scrollTo(coords.x, coords.y), {
        x,
        y,
      });
    }

    return { scrolled: { x, y, uid, selector } };
  },

  async focus(ctx, params, executor) {
    const { uid, selector } = params;

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.focus();
      return { focused: uid };
    }

    if (selector) {
      const page = ctx.getPage();
      await page.focus(selector);
      return { focused: selector };
    }

    throw new Error("focus 需要 uid 或 selector");
  },

  async clear(ctx, params, executor) {
    let { uid, selector } = params;

    if (uid) {
      const element = await ctx.getHandleByUid(uid);
      await element.click({ clickCount: 3 });
      await element.press("Backspace");
      return { cleared: uid };
    }

    if (selector) {
      const page = ctx.getPage();
      await page.click(selector, { clickCount: 3 });
      await page.keyboard.press("Backspace");
      return { cleared: selector };
    }

    throw new Error("clear 需要 uid 或 selector");
  },

  async reload(ctx, params, executor) {
    const page = ctx.getPage();
    await page.reload({ waitUntil: params.waitUntil || "domcontentloaded" });
    executor.log("info", "页面已刷新");
    return { reloaded: true };
  },

  async goBack(ctx, params, executor) {
    const page = ctx.getPage();
    await page.goBack({ waitUntil: params.waitUntil || "domcontentloaded" });
    executor.log("info", "返回上一页");
    return { url: page.url() };
  },

  async goForward(ctx, params, executor) {
    const page = ctx.getPage();
    await page.goForward({ waitUntil: params.waitUntil || "domcontentloaded" });
    executor.log("info", "前进下一页");
    return { url: page.url() };
  },
};

function getActionHandler(actionName) {
  return actionHandlers[actionName] || null;
}

function listActions() {
  return Object.keys(actionHandlers);
}

module.exports = {
  actionHandlers,
  getActionHandler,
  listActions,
};
