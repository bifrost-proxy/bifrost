const DEVICE_PRESETS = {
  'iPhone SE': { width: 375, height: 667, deviceScaleFactor: 2, isMobile: true },
  'iPhone XR': { width: 414, height: 896, deviceScaleFactor: 2, isMobile: true },
  'iPhone 12 Pro': { width: 390, height: 844, deviceScaleFactor: 3, isMobile: true },
  'iPhone 14': { width: 390, height: 844, deviceScaleFactor: 3, isMobile: true },
  'iPhone 14 Pro': { width: 393, height: 852, deviceScaleFactor: 3, isMobile: true },
  'iPhone 14 Pro Max': { width: 430, height: 932, deviceScaleFactor: 3, isMobile: true },
  'Pixel 5': { width: 393, height: 851, deviceScaleFactor: 2.75, isMobile: true },
  'Pixel 7 Pro': { width: 412, height: 915, deviceScaleFactor: 3.5, isMobile: true },
  'Samsung Galaxy S8+': { width: 360, height: 740, deviceScaleFactor: 4, isMobile: true },
  'Samsung Galaxy S20 Ultra': { width: 412, height: 915, deviceScaleFactor: 3.5, isMobile: true },
  'Samsung Galaxy S23': { width: 360, height: 780, deviceScaleFactor: 3, isMobile: true },
  'iPad Air': { width: 820, height: 1180, deviceScaleFactor: 2, isMobile: true },
  'iPad Mini': { width: 768, height: 1024, deviceScaleFactor: 2, isMobile: true },
  'iPad Pro 11': { width: 834, height: 1194, deviceScaleFactor: 2, isMobile: true },
  'iPad Pro 12.9': { width: 1024, height: 1366, deviceScaleFactor: 2, isMobile: true },
  'Surface Pro 7': { width: 912, height: 1368, deviceScaleFactor: 2, isMobile: true },
  'Galaxy Fold': { width: 280, height: 653, deviceScaleFactor: 3, isMobile: true }
};

async function setViewport(ctx, params = {}) {
  const { width, height, deviceScaleFactor = 1, isMobile = false, hasTouch = false } = params;
  
  if (!width || !height) {
    throw new Error('width and height are required');
  }
  
  await ctx.page.setViewport({
    width,
    height,
    deviceScaleFactor,
    isMobile,
    hasTouch
  });
  
  return { viewport: { width, height, deviceScaleFactor, isMobile } };
}

async function emulateDevice(ctx, params = {}) {
  const { device } = params;
  
  if (!device) {
    throw new Error('device is required');
  }
  
  const preset = DEVICE_PRESETS[device];
  
  if (!preset) {
    throw new Error(`Unknown device: ${device}. Available: ${Object.keys(DEVICE_PRESETS).join(', ')}`);
  }
  
  await ctx.page.setViewport({
    width: preset.width,
    height: preset.height,
    deviceScaleFactor: preset.deviceScaleFactor,
    isMobile: preset.isMobile,
    hasTouch: preset.isMobile
  });
  
  return { device, viewport: preset };
}

async function setGeolocation(ctx, params = {}) {
  const { latitude, longitude, accuracy = 100 } = params;
  
  if (latitude === undefined || longitude === undefined) {
    throw new Error('latitude and longitude are required');
  }
  
  const browserContext = ctx.page.browserContext();
  await browserContext.overridePermissions(ctx.page.url(), ['geolocation']);
  await ctx.page.setGeolocation({ latitude, longitude, accuracy });
  
  return { geolocation: { latitude, longitude, accuracy } };
}

async function setPermissions(ctx, params = {}) {
  const { permissions, origin } = params;
  
  if (!permissions || !Array.isArray(permissions)) {
    throw new Error('permissions array is required');
  }
  
  const browserContext = ctx.page.browserContext();
  const targetOrigin = origin || ctx.page.url();
  await browserContext.overridePermissions(targetOrigin, permissions);
  
  return { permissions, origin: targetOrigin };
}

async function setOffline(ctx, params = {}) {
  const { offline } = params;
  
  await ctx.page.setOfflineMode(offline);
  
  return { offline };
}

async function setTimezone(ctx, params = {}) {
  const { timezoneId } = params;
  
  if (!timezoneId) {
    throw new Error('timezoneId is required');
  }
  
  await ctx.page.emulateTimezone(timezoneId);
  
  return { timezoneId };
}

async function setColorScheme(ctx, params = {}) {
  const { colorScheme } = params;
  
  if (!colorScheme) {
    throw new Error('colorScheme is required (light or dark)');
  }
  
  await ctx.page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: colorScheme }]);
  
  return { colorScheme };
}

async function setCPUThrottling(ctx, params = {}) {
  const { rate = 4 } = params;
  
  const client = await ctx.page.target().createCDPSession();
  await client.send('Emulation.setCPUThrottlingRate', { rate });
  
  return { rate };
}

async function setNetworkConditions(ctx, params = {}) {
  const {
    download = -1,
    upload = -1,
    latency = 0,
    offline = false
  } = params;
  
  const client = await ctx.page.target().createCDPSession();
  await client.send('Network.emulateNetworkConditions', {
    offline,
    latency,
    downloadThroughput: download,
    uploadThroughput: upload
  });
  
  return { download, upload, latency, offline };
}

async function setSlowNetwork(ctx, params = {}) {
  const { type = '3G' } = params;
  
  const presets = {
    '3G': { download: 375000, upload: 75000, latency: 400 },
    'Slow 3G': { download: 50000, upload: 50000, latency: 2000 },
    'Fast 3G': { download: 187500, upload: 93750, latency: 562 },
    '4G': { download: 4000000, upload: 3000000, latency: 100 }
  };
  
  const conditions = presets[type];
  
  if (!conditions) {
    throw new Error(`Unknown preset: ${type}. Available: ${Object.keys(presets).join(', ')}`);
  }
  
  const client = await ctx.page.target().createCDPSession();
  await client.send('Network.emulateNetworkConditions', {
    offline: false,
    latency: conditions.latency,
    downloadThroughput: conditions.download,
    uploadThroughput: conditions.upload
  });
  
  return { type, conditions };
}

async function getAvailableDevices(ctx, params = {}) {
  return {
    devices: Object.entries(DEVICE_PRESETS).map(([name, preset]) => ({
      name,
      ...preset
    }))
  };
}

module.exports = {
  DEVICE_PRESETS,
  setViewport,
  emulateDevice,
  setGeolocation,
  setPermissions,
  setOffline,
  setTimezone,
  setColorScheme,
  setCPUThrottling,
  setNetworkConditions,
  setSlowNetwork,
  getAvailableDevices
};
