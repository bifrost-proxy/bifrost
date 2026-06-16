const fs = require('fs');
const path = require('path');
const { SCREENSHOT_DIR } = require('../config');

async function takeSnapshot(ctx, params = {}) {
  const snapshot = await ctx.createSnapshot();
  const formatted = snapshot.formatSnapshot();
  
  return {
    formatted,
    nodeCount: snapshot.nodeCount,
    timestamp: Date.now()
  };
}

async function getSnapshotJSON(ctx, params = {}) {
  const snapshot = await ctx.createSnapshot();
  return snapshot.toJSON();
}

async function findElements(ctx, params = {}) {
  const { query, role, name, refresh = false } = params;
  const snapshot = refresh ? await ctx.createSnapshot() : await ctx.getOrCreateSnapshot();
  
  let results = [];
  
  if (role) {
    results = snapshot.findNodesByRole(role);
  } else if (name) {
    results = snapshot.findNodesByName(name);
  } else if (query) {
    results = snapshot.findNodes(query);
  }
  
  return results.map(node => ({
    uid: node.id,
    role: node.role,
    name: node.name,
    value: node.value
  }));
}

async function getInteractiveElements(ctx, params = {}) {
  const { refresh = false } = params;
  const snapshot = refresh ? await ctx.createSnapshot() : await ctx.getOrCreateSnapshot();
  const elements = snapshot.findInteractiveElements();
  
  return elements.map(node => ({
    uid: node.id,
    role: node.role,
    name: node.name,
    disabled: node.disabled
  }));
}

async function screenshot(ctx, params = {}) {
  const { name, fullPage = false, type = 'png', quality } = params;
  
  if (!fs.existsSync(SCREENSHOT_DIR)) {
    fs.mkdirSync(SCREENSHOT_DIR, { recursive: true });
  }
  
  const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
  const filename = name ? `${name}.${type}` : `screenshot-${timestamp}.${type}`;
  const filepath = path.join(SCREENSHOT_DIR, filename);
  
  const options = {
    path: filepath,
    fullPage,
    type
  };
  
  if (quality && type !== 'png') {
    options.quality = quality;
  }
  
  const page = ctx.getPage();
  await page.screenshot(options);
  
  return {
    path: filepath,
    fullPage,
    type
  };
}

async function getElementInfo(ctx, params = {}) {
  const { uid, selector } = params;
  
  let element;
  let node = null;
  
  if (uid) {
    const snapshot = await ctx.getOrCreateSnapshot();
    node = snapshot.getElementByUid(uid);
    if (!node) {
      const suggestions = snapshot.getSuggestions(uid);
      throw new Error(`Element with UID '${uid}' not found. ${suggestions.length > 0 ? `Did you mean: ${suggestions.map(s => s.uid).join(', ')}?` : ''}`);
    }
    element = await ctx.getHandleByUid(uid);
  } else if (selector) {
    const page = ctx.getPage();
    element = await page.$(selector);
    if (!element) {
      throw new Error(`Element not found: ${selector}`);
    }
  } else {
    throw new Error('Either uid or selector is required');
  }
  
  const boundingBox = await element.boundingBox();
  const isVisible = await element.isVisible();
  const isEnabled = await element.isEnabled();
  
  const tagName = await element.evaluate(el => el.tagName.toLowerCase());
  const textContent = await element.evaluate(el => el.textContent?.trim()?.substring(0, 100) || '');
  
  return {
    uid: node?.id,
    role: node?.role,
    name: node?.name,
    tagName,
    textContent,
    boundingBox,
    isVisible,
    isEnabled
  };
}

module.exports = {
  takeSnapshot,
  getSnapshotJSON,
  findElements,
  getInteractiveElements,
  screenshot,
  getElementInfo
};
