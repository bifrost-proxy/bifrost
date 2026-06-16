const KEYBOARD_MODIFIERS = ['Alt', 'Control', 'Meta', 'Shift'];

async function click(ctx, params = {}) {
  const { uid, selector } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.click();
  
  return { clicked: true, uid, selector };
}

async function clickAt(ctx, params = {}) {
  const { x, y } = params;
  if (x === undefined || y === undefined) {
    throw new Error('x and y coordinates are required');
  }
  
  await ctx.page.mouse.click(x, y);
  
  return { clicked: true, x, y };
}

async function doubleClick(ctx, params = {}) {
  const { uid, selector } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.click({ clickCount: 2 });
  
  return { doubleClicked: true, uid, selector };
}

async function hover(ctx, params = {}) {
  const { uid, selector } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.hover();
  
  return { hovered: true, uid, selector };
}

async function fill(ctx, params = {}) {
  const { uid, selector, value, clear = true } = params;
  
  if (value === undefined) {
    throw new Error('value is required');
  }
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  if (clear) {
    await element.click({ clickCount: 3 });
  }
  
  await element.type(value);
  
  return { filled: true, uid, selector, valueLength: value.length };
}

async function fillForm(ctx, params = {}) {
  const { fields } = params;
  
  if (!Array.isArray(fields)) {
    throw new Error('fields must be an array');
  }
  
  const results = [];
  for (const field of fields) {
    const result = await fill(ctx, field);
    results.push(result);
  }
  
  return { filled: results.length, results };
}

async function select(ctx, params = {}) {
  const { uid, selector, value } = params;
  
  if (value === undefined) {
    throw new Error('value is required');
  }
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.select(value);
  
  return { selected: true, value };
}

async function drag(ctx, params = {}) {
  const { uid, selector, x, y } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  const box = await element.boundingBox();
  if (!box) throw new Error('Could not get element bounding box');
  
  const startX = box.x + box.width / 2;
  const startY = box.y + box.height / 2;
  
  await ctx.page.mouse.move(startX, startY);
  await ctx.page.mouse.down();
  await ctx.page.mouse.move(x, y, { steps: 10 });
  await ctx.page.mouse.up();
  
  return { dragged: true, from: { x: startX, y: startY }, to: { x, y } };
}

async function dragTo(ctx, params = {}) {
  const { source, target } = params;
  
  if (!source || !target) {
    throw new Error('source and target are required');
  }
  
  const sourceEl = source.uid
    ? await ctx.getElementByUid(source.uid)
    : await ctx.page.$(source.selector);
  const targetEl = target.uid
    ? await ctx.getElementByUid(target.uid)
    : await ctx.page.$(target.selector);
  
  if (!sourceEl) throw new Error('Source element not found');
  if (!targetEl) throw new Error('Target element not found');
  
  const sourceBox = await sourceEl.boundingBox();
  const targetBox = await targetEl.boundingBox();
  
  if (!sourceBox || !targetBox) throw new Error('Could not get bounding boxes');
  
  const startX = sourceBox.x + sourceBox.width / 2;
  const startY = sourceBox.y + sourceBox.height / 2;
  const endX = targetBox.x + targetBox.width / 2;
  const endY = targetBox.y + targetBox.height / 2;
  
  await ctx.page.mouse.move(startX, startY);
  await ctx.page.mouse.down();
  await ctx.page.mouse.move(endX, endY, { steps: 10 });
  await ctx.page.mouse.up();
  
  return { dragged: true, from: { x: startX, y: startY }, to: { x: endX, y: endY } };
}

async function uploadFile(ctx, params = {}) {
  const { uid, selector, file, files } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  const filePaths = files || (file ? [file] : []);
  if (filePaths.length === 0) {
    throw new Error('file or files is required');
  }
  
  await element.uploadFile(...filePaths);
  
  return { uploaded: filePaths.length, files: filePaths };
}

async function pressKey(ctx, params = {}) {
  const { key, modifiers = [] } = params;
  
  if (!key) {
    throw new Error('key is required');
  }
  
  for (const mod of modifiers) {
    if (KEYBOARD_MODIFIERS.includes(mod)) {
      await ctx.page.keyboard.down(mod);
    }
  }
  
  await ctx.page.keyboard.press(key);
  
  for (const mod of [...modifiers].reverse()) {
    if (KEYBOARD_MODIFIERS.includes(mod)) {
      await ctx.page.keyboard.up(mod);
    }
  }
  
  return { pressed: true, key, modifiers };
}

async function type(ctx, params = {}) {
  const { text, delay = 0 } = params;
  
  if (!text) {
    throw new Error('text is required');
  }
  
  await ctx.page.keyboard.type(text, { delay });
  
  return { typed: true, length: text.length };
}

async function focus(ctx, params = {}) {
  const { uid, selector } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.focus();
  
  return { focused: true };
}

async function blur(ctx, params = {}) {
  const { uid, selector } = params;
  
  if (uid || selector) {
    let element;
    if (uid) {
      element = await ctx.getElementByUid(uid);
    } else {
      element = await ctx.page.$(selector);
    }
    if (element) {
      await element.evaluate(el => el.blur());
    }
  } else {
    await ctx.page.evaluate(() => {
      if (document.activeElement && document.activeElement !== document.body) {
        document.activeElement.blur();
      }
    });
  }
  
  return { blurred: true };
}

async function scroll(ctx, params = {}) {
  const { x = 0, y = 0, uid, selector } = params;
  
  if (uid || selector) {
    let element;
    if (uid) {
      element = await ctx.getElementByUid(uid);
    } else {
      element = await ctx.page.$(selector);
    }
    if (element) {
      await element.evaluate((el, dx, dy) => el.scrollBy(dx, dy), x, y);
    }
  } else {
    await ctx.page.evaluate((dx, dy) => window.scrollBy(dx, dy), x, y);
  }
  
  return { scrolled: true, x, y };
}

async function scrollIntoView(ctx, params = {}) {
  const { uid, selector, block = 'center' } = params;
  
  let element;
  if (uid) {
    element = await ctx.getElementByUid(uid);
  } else if (selector) {
    element = await ctx.page.$(selector);
    if (!element) throw new Error(`Element not found: ${selector}`);
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  await element.evaluate((el, b) => el.scrollIntoView({ behavior: 'smooth', block: b }), block);
  
  return { scrolledIntoView: true };
}

module.exports = {
  click,
  clickAt,
  doubleClick,
  hover,
  fill,
  fillForm,
  select,
  drag,
  dragTo,
  uploadFile,
  pressKey,
  type,
  focus,
  blur,
  scroll,
  scrollIntoView
};
