const inputTools = require('./input');
const snapshotTools = require('./snapshot');
const navigationTools = require('./navigation');
const assertTools = require('./assert');
const networkTools = require('./network');
const emulationTools = require('./emulation');
const debuggingTools = require('./debugging');

const tools = {
  click: {
    name: 'click',
    description: 'Click on an element by UID or selector',
    category: 'input',
    params: { uid: 'string?', selector: 'string?' },
    handler: inputTools.click
  },
  clickAt: {
    name: 'clickAt',
    description: 'Click at specific coordinates',
    category: 'input',
    params: { x: 'number', y: 'number' },
    handler: inputTools.clickAt
  },
  doubleClick: {
    name: 'doubleClick',
    description: 'Double-click on an element',
    category: 'input',
    params: { uid: 'string?', selector: 'string?' },
    handler: inputTools.doubleClick
  },
  hover: {
    name: 'hover',
    description: 'Hover over an element',
    category: 'input',
    params: { uid: 'string?', selector: 'string?' },
    handler: inputTools.hover
  },
  fill: {
    name: 'fill',
    description: 'Fill an input field with text',
    category: 'input',
    params: { uid: 'string?', selector: 'string?', value: 'string', clear: 'boolean?' },
    handler: inputTools.fill
  },
  fillForm: {
    name: 'fillForm',
    description: 'Fill multiple form fields at once',
    category: 'input',
    params: { fields: 'array' },
    handler: inputTools.fillForm
  },
  select: {
    name: 'select',
    description: 'Select an option from a dropdown',
    category: 'input',
    params: { uid: 'string?', selector: 'string?', value: 'string' },
    handler: inputTools.select
  },
  drag: {
    name: 'drag',
    description: 'Drag an element to coordinates',
    category: 'input',
    params: { uid: 'string?', selector: 'string?', x: 'number', y: 'number' },
    handler: inputTools.drag
  },
  dragTo: {
    name: 'dragTo',
    description: 'Drag an element to another element',
    category: 'input',
    params: { source: 'object', target: 'object' },
    handler: inputTools.dragTo
  },
  uploadFile: {
    name: 'uploadFile',
    description: 'Upload file(s) to an input element',
    category: 'input',
    params: { uid: 'string?', selector: 'string?', file: 'string?', files: 'array?' },
    handler: inputTools.uploadFile
  },
  pressKey: {
    name: 'pressKey',
    description: 'Press a keyboard key',
    category: 'input',
    params: { key: 'string', modifiers: 'array?' },
    handler: inputTools.pressKey
  },
  type: {
    name: 'type',
    description: 'Type text using keyboard',
    category: 'input',
    params: { text: 'string', delay: 'number?' },
    handler: inputTools.type
  },
  focus: {
    name: 'focus',
    description: 'Focus an element',
    category: 'input',
    params: { uid: 'string?', selector: 'string?' },
    handler: inputTools.focus
  },
  blur: {
    name: 'blur',
    description: 'Remove focus from element',
    category: 'input',
    params: { uid: 'string?', selector: 'string?' },
    handler: inputTools.blur
  },
  scroll: {
    name: 'scroll',
    description: 'Scroll the page or element',
    category: 'input',
    params: { x: 'number?', y: 'number?', uid: 'string?', selector: 'string?' },
    handler: inputTools.scroll
  },
  scrollIntoView: {
    name: 'scrollIntoView',
    description: 'Scroll element into viewport',
    category: 'input',
    params: { uid: 'string?', selector: 'string?', block: 'string?' },
    handler: inputTools.scrollIntoView
  },

  takeSnapshot: {
    name: 'takeSnapshot',
    description: 'Take accessibility tree snapshot of the page',
    category: 'snapshot',
    params: {},
    handler: snapshotTools.takeSnapshot
  },
  getSnapshotJSON: {
    name: 'getSnapshotJSON',
    description: 'Get snapshot as JSON object',
    category: 'snapshot',
    params: {},
    handler: snapshotTools.getSnapshotJSON
  },
  findElements: {
    name: 'findElements',
    description: 'Find elements by query, role, or name',
    category: 'snapshot',
    params: { query: 'string?', role: 'string?', name: 'string?' },
    handler: snapshotTools.findElements
  },
  getInteractiveElements: {
    name: 'getInteractiveElements',
    description: 'Get all interactive elements on the page',
    category: 'snapshot',
    params: {},
    handler: snapshotTools.getInteractiveElements
  },
  screenshot: {
    name: 'screenshot',
    description: 'Take a screenshot of the page',
    category: 'snapshot',
    params: { name: 'string?', fullPage: 'boolean?', type: 'string?', quality: 'number?' },
    handler: snapshotTools.screenshot
  },
  getElementInfo: {
    name: 'getElementInfo',
    description: 'Get detailed information about an element',
    category: 'snapshot',
    params: { uid: 'string?', selector: 'string?' },
    handler: snapshotTools.getElementInfo
  },

  navigate: {
    name: 'navigate',
    description: 'Navigate to a URL',
    category: 'navigation',
    params: { url: 'string', waitUntil: 'string?' },
    handler: navigationTools.navigate
  },
  goBack: {
    name: 'goBack',
    description: 'Go back in browser history',
    category: 'navigation',
    params: { waitUntil: 'string?' },
    handler: navigationTools.goBack
  },
  goForward: {
    name: 'goForward',
    description: 'Go forward in browser history',
    category: 'navigation',
    params: { waitUntil: 'string?' },
    handler: navigationTools.goForward
  },
  reload: {
    name: 'reload',
    description: 'Reload the current page',
    category: 'navigation',
    params: { waitUntil: 'string?' },
    handler: navigationTools.reload
  },
  waitForNavigation: {
    name: 'waitForNavigation',
    description: 'Wait for navigation to complete',
    category: 'navigation',
    params: { waitUntil: 'string?', timeout: 'number?' },
    handler: navigationTools.waitForNavigation
  },
  waitForLoad: {
    name: 'waitForLoad',
    description: 'Wait for page to fully load',
    category: 'navigation',
    params: { state: 'string?', timeout: 'number?' },
    handler: navigationTools.waitForLoad
  },
  getPageInfo: {
    name: 'getPageInfo',
    description: 'Get current page information',
    category: 'navigation',
    params: {},
    handler: navigationTools.getPageInfo
  },
  waitForUrl: {
    name: 'waitForUrl',
    description: 'Wait for URL to match pattern',
    category: 'navigation',
    params: { url: 'string?', pattern: 'string?', timeout: 'number?', partial: 'boolean?' },
    handler: navigationTools.waitForUrl
  },
  setExtraHTTPHeaders: {
    name: 'setExtraHTTPHeaders',
    description: 'Set extra HTTP headers for all requests',
    category: 'navigation',
    params: { headers: 'object' },
    handler: navigationTools.setExtraHTTPHeaders
  },
  setUserAgent: {
    name: 'setUserAgent',
    description: 'Set user agent string',
    category: 'navigation',
    params: { userAgent: 'string' },
    handler: navigationTools.setUserAgent
  },

  assertVisible: {
    name: 'assertVisible',
    description: 'Assert element is visible',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', timeout: 'number?' },
    handler: assertTools.assertVisible
  },
  assertHidden: {
    name: 'assertHidden',
    description: 'Assert element is hidden',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', timeout: 'number?' },
    handler: assertTools.assertHidden
  },
  assertText: {
    name: 'assertText',
    description: 'Assert element text content',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', text: 'string', contains: 'boolean?' },
    handler: assertTools.assertText
  },
  assertValue: {
    name: 'assertValue',
    description: 'Assert input element value',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', value: 'string' },
    handler: assertTools.assertValue
  },
  assertChecked: {
    name: 'assertChecked',
    description: 'Assert checkbox/radio checked state',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', checked: 'boolean?' },
    handler: assertTools.assertChecked
  },
  assertDisabled: {
    name: 'assertDisabled',
    description: 'Assert element disabled state',
    category: 'assert',
    params: { uid: 'string?', selector: 'string?', disabled: 'boolean?' },
    handler: assertTools.assertDisabled
  },
  assertCount: {
    name: 'assertCount',
    description: 'Assert number of matching elements',
    category: 'assert',
    params: { selector: 'string', count: 'number' },
    handler: assertTools.assertCount
  },
  assertPageTitle: {
    name: 'assertPageTitle',
    description: 'Assert page title',
    category: 'assert',
    params: { title: 'string', contains: 'boolean?' },
    handler: assertTools.assertPageTitle
  },
  assertUrl: {
    name: 'assertUrl',
    description: 'Assert current URL',
    category: 'assert',
    params: { url: 'string?', pattern: 'string?', partial: 'boolean?' },
    handler: assertTools.assertUrl
  },
  assertNoErrors: {
    name: 'assertNoErrors',
    description: 'Assert no console errors',
    category: 'assert',
    params: { ignorePatterns: 'array?' },
    handler: assertTools.assertNoErrors
  },

  getNetworkRequests: {
    name: 'getNetworkRequests',
    description: 'Get captured network requests',
    category: 'network',
    params: { urlPattern: 'string?', status: 'number?', method: 'string?', resourceType: 'string?', limit: 'number?' },
    handler: networkTools.getNetworkRequests
  },
  getRequestContent: {
    name: 'getRequestContent',
    description: 'Get full details of a network request',
    category: 'network',
    params: { id: 'string' },
    handler: networkTools.getRequestContent
  },
  getFailedRequests: {
    name: 'getFailedRequests',
    description: 'Get failed network requests',
    category: 'network',
    params: { limit: 'number?' },
    handler: networkTools.getFailedRequests
  },
  waitForRequest: {
    name: 'waitForRequest',
    description: 'Wait for a specific request',
    category: 'network',
    params: { url: 'string?', urlPattern: 'string?', timeout: 'number?' },
    handler: networkTools.waitForRequest
  },
  waitForResponse: {
    name: 'waitForResponse',
    description: 'Wait for a specific response',
    category: 'network',
    params: { url: 'string?', urlPattern: 'string?', timeout: 'number?' },
    handler: networkTools.waitForResponse
  },
  clearNetworkRequests: {
    name: 'clearNetworkRequests',
    description: 'Clear captured network requests',
    category: 'network',
    params: {},
    handler: networkTools.clearNetworkRequests
  },
  setRequestInterception: {
    name: 'setRequestInterception',
    description: 'Enable/disable request interception',
    category: 'network',
    params: { enabled: 'boolean' },
    handler: networkTools.setRequestInterception
  },
  mockRequest: {
    name: 'mockRequest',
    description: 'Mock a network request',
    category: 'network',
    params: { url: 'string?', urlPattern: 'string?', response: 'object', once: 'boolean?' },
    handler: networkTools.mockRequest
  },
  blockRequests: {
    name: 'blockRequests',
    description: 'Block requests matching patterns',
    category: 'network',
    params: { patterns: 'array?', resourceTypes: 'array?' },
    handler: networkTools.blockRequests
  },
  getNetworkStats: {
    name: 'getNetworkStats',
    description: 'Get network statistics',
    category: 'network',
    params: {},
    handler: networkTools.getNetworkStats
  },

  setViewport: {
    name: 'setViewport',
    description: 'Set viewport size',
    category: 'emulation',
    params: { width: 'number', height: 'number', deviceScaleFactor: 'number?', isMobile: 'boolean?', hasTouch: 'boolean?' },
    handler: emulationTools.setViewport
  },
  emulateDevice: {
    name: 'emulateDevice',
    description: 'Emulate a mobile device',
    category: 'emulation',
    params: { device: 'string' },
    handler: emulationTools.emulateDevice
  },
  setGeolocation: {
    name: 'setGeolocation',
    description: 'Set geolocation coordinates',
    category: 'emulation',
    params: { latitude: 'number', longitude: 'number', accuracy: 'number?' },
    handler: emulationTools.setGeolocation
  },
  setPermissions: {
    name: 'setPermissions',
    description: 'Set browser permissions',
    category: 'emulation',
    params: { permissions: 'array', origin: 'string?' },
    handler: emulationTools.setPermissions
  },
  setOffline: {
    name: 'setOffline',
    description: 'Set offline mode',
    category: 'emulation',
    params: { offline: 'boolean' },
    handler: emulationTools.setOffline
  },
  setTimezone: {
    name: 'setTimezone',
    description: 'Set timezone',
    category: 'emulation',
    params: { timezoneId: 'string' },
    handler: emulationTools.setTimezone
  },
  setColorScheme: {
    name: 'setColorScheme',
    description: 'Set color scheme (light/dark)',
    category: 'emulation',
    params: { colorScheme: 'string' },
    handler: emulationTools.setColorScheme
  },
  setCPUThrottling: {
    name: 'setCPUThrottling',
    description: 'Set CPU throttling rate',
    category: 'emulation',
    params: { rate: 'number?' },
    handler: emulationTools.setCPUThrottling
  },
  setNetworkConditions: {
    name: 'setNetworkConditions',
    description: 'Set network conditions',
    category: 'emulation',
    params: { download: 'number?', upload: 'number?', latency: 'number?', offline: 'boolean?' },
    handler: emulationTools.setNetworkConditions
  },
  setSlowNetwork: {
    name: 'setSlowNetwork',
    description: 'Set slow network preset (3G, 4G, etc)',
    category: 'emulation',
    params: { type: 'string?' },
    handler: emulationTools.setSlowNetwork
  },
  getAvailableDevices: {
    name: 'getAvailableDevices',
    description: 'Get list of available device presets',
    category: 'emulation',
    params: {},
    handler: emulationTools.getAvailableDevices
  },

  getConsoleLogs: {
    name: 'getConsoleLogs',
    description: 'Get console log messages',
    category: 'debugging',
    params: { level: 'string?', limit: 'number?' },
    handler: debuggingTools.getConsoleLogs
  },
  getConsoleErrors: {
    name: 'getConsoleErrors',
    description: 'Get console error messages',
    category: 'debugging',
    params: { limit: 'number?' },
    handler: debuggingTools.getConsoleErrors
  },
  getConsoleWarnings: {
    name: 'getConsoleWarnings',
    description: 'Get console warning messages',
    category: 'debugging',
    params: { limit: 'number?' },
    handler: debuggingTools.getConsoleWarnings
  },
  clearConsoleLogs: {
    name: 'clearConsoleLogs',
    description: 'Clear captured console logs',
    category: 'debugging',
    params: {},
    handler: debuggingTools.clearConsoleLogs
  },
  evaluateScript: {
    name: 'evaluateScript',
    description: 'Evaluate JavaScript in page context',
    category: 'debugging',
    params: { script: 'string' },
    handler: debuggingTools.evaluateScript
  },
  evaluateOnElement: {
    name: 'evaluateOnElement',
    description: 'Evaluate JavaScript on an element',
    category: 'debugging',
    params: { uid: 'string?', selector: 'string?', script: 'string' },
    handler: debuggingTools.evaluateOnElement
  },
  getPageMetrics: {
    name: 'getPageMetrics',
    description: 'Get page performance metrics',
    category: 'debugging',
    params: {},
    handler: debuggingTools.getPageMetrics
  },
  getPerformanceTiming: {
    name: 'getPerformanceTiming',
    description: 'Get performance timing data',
    category: 'debugging',
    params: {},
    handler: debuggingTools.getPerformanceTiming
  },
  getCoverage: {
    name: 'getCoverage',
    description: 'Get code coverage data',
    category: 'debugging',
    params: { type: 'string?' },
    handler: debuggingTools.getCoverage
  },
  startCoverage: {
    name: 'startCoverage',
    description: 'Start code coverage collection',
    category: 'debugging',
    params: { type: 'string?' },
    handler: debuggingTools.startCoverage
  },
  getStatus: {
    name: 'getStatus',
    description: 'Get current test context status',
    category: 'debugging',
    params: {},
    handler: debuggingTools.getStatus
  },
  waitForDebugger: {
    name: 'waitForDebugger',
    description: 'Pause execution for debugger',
    category: 'debugging',
    params: { timeout: 'number?' },
    handler: debuggingTools.waitForDebugger
  },
  resumeDebugger: {
    name: 'resumeDebugger',
    description: 'Resume from debugger pause',
    category: 'debugging',
    params: {},
    handler: debuggingTools.resumeDebugger
  }
};

const categories = {
  input: ['click', 'clickAt', 'doubleClick', 'hover', 'fill', 'fillForm', 'select', 'drag', 'dragTo', 'uploadFile', 'pressKey', 'type', 'focus', 'blur', 'scroll', 'scrollIntoView'],
  snapshot: ['takeSnapshot', 'getSnapshotJSON', 'findElements', 'getInteractiveElements', 'screenshot', 'getElementInfo'],
  navigation: ['navigate', 'goBack', 'goForward', 'reload', 'waitForNavigation', 'waitForLoad', 'getPageInfo', 'waitForUrl', 'setExtraHTTPHeaders', 'setUserAgent'],
  assert: ['assertVisible', 'assertHidden', 'assertText', 'assertValue', 'assertChecked', 'assertDisabled', 'assertCount', 'assertPageTitle', 'assertUrl', 'assertNoErrors'],
  network: ['getNetworkRequests', 'getRequestContent', 'getFailedRequests', 'waitForRequest', 'waitForResponse', 'clearNetworkRequests', 'setRequestInterception', 'mockRequest', 'blockRequests', 'getNetworkStats'],
  emulation: ['setViewport', 'emulateDevice', 'setGeolocation', 'setPermissions', 'setOffline', 'setTimezone', 'setColorScheme', 'setCPUThrottling', 'setNetworkConditions', 'setSlowNetwork', 'getAvailableDevices'],
  debugging: ['getConsoleLogs', 'getConsoleErrors', 'getConsoleWarnings', 'clearConsoleLogs', 'evaluateScript', 'evaluateOnElement', 'getPageMetrics', 'getPerformanceTiming', 'getCoverage', 'startCoverage', 'getStatus', 'waitForDebugger', 'resumeDebugger']
};

function getToolByName(name) {
  return tools[name] || null;
}

function getToolsByCategory(category) {
  const toolNames = categories[category];
  if (!toolNames) return [];
  return toolNames.map(name => tools[name]).filter(Boolean);
}

function listTools() {
  const result = {};
  for (const [category, toolNames] of Object.entries(categories)) {
    result[category] = toolNames.map(name => {
      const tool = tools[name];
      return {
        name: tool.name,
        description: tool.description,
        params: tool.params
      };
    });
  }
  return result;
}

module.exports = {
  tools,
  categories,
  getToolByName,
  getToolsByCategory,
  listTools
};
