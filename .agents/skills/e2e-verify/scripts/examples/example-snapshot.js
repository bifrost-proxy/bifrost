const {
  launchBrowser,
  closeBrowser,
  setupPageListeners,
  createTestContext,
  executeTool,
  handleSSOIfNeeded,
} = require("../browser-test");
const { NetworkCollector } = require("../lib/network-collector");
const fs = require("fs");
const path = require("path");

const logsDir = path.join(__dirname, "logs");
if (!fs.existsSync(logsDir)) {
  fs.mkdirSync(logsDir, { recursive: true });
}

const timestamp = new Date().toISOString().replace(/[:.]/g, "-");
const logFile = path.join(logsDir, `test-snapshot-${timestamp}.log`);
const networkLogFile = path.join(logsDir, `network-requests-${timestamp}.json`);

function log(message) {
  const ts = new Date().toISOString();
  const logMessage = `[${ts}] ${message}`;
  console.log(logMessage);
  fs.appendFileSync(logFile, logMessage + "\n");
}

async function main() {
  log("=== Test Snapshot Script Started ===");
  log(`Log file: ${logFile}`);
  log(`Network log file: ${networkLogFile}`);

  log("Starting browser...");
  const browser = await launchBrowser({ headless: false });
  const page = await browser.newPage();
  setupPageListeners(page);

  const networkCollector = new NetworkCollector();
  networkCollector.attach(page);
  networkCollector.start();

  log("Creating test context...");
  const ctx = await createTestContext(page, { timeout: 30000 });

  log("Navigating to http://localhost:9900...");
  await executeTool(ctx, "navigate", { url: "http://localhost:9900" });

  log("Waiting for page to load...");
  await new Promise((resolve) => setTimeout(resolve, 3000));

  const handled = await handleSSOIfNeeded(page, ctx, { timeout: 120000 });
  if (handled) {
    log("Waiting for homepage to fully load...");
    await new Promise((resolve) => setTimeout(resolve, 5000));
  }

  log("Taking snapshot...");
  const snapshot = await executeTool(ctx, "takeSnapshot", {});

  log("\n=== SNAPSHOT RESULT ===\n");
  log(`Success: ${snapshot.success}`);
  log("\n--- Formatted Output ---\n");
  const formatted = snapshot.result?.formatted || "No formatted output";
  log(formatted);
  log("\n--- Raw Result Keys ---\n");
  log(JSON.stringify(Object.keys(snapshot.result || {})));

  if (snapshot.result?.raw) {
    log("\n--- Raw Structure (first 3 items) ---\n");
    log(JSON.stringify(snapshot.result.raw.slice(0, 3), null, 2));
  }

  log("\n=== END SNAPSHOT ===\n");

  const summary = networkCollector.getSummary();
  const apiRequests = networkCollector.getApiRequests();
  const failedRequests = networkCollector.getFailedRequests();

  log("\n=== NETWORK REQUESTS SUMMARY ===\n");
  log(`Total requests: ${summary.total}`);
  log(`API requests: ${apiRequests.length}`);
  log(`Failed requests: ${failedRequests.length}`);

  if (apiRequests.length > 0) {
    log("\n--- API Requests ---\n");
    apiRequests.forEach((r, i) => {
      const status = r.status ? `[${r.status}]` : "[pending]";
      const duration = r.duration ? `${r.duration}ms` : "N/A";
      log(
        `  ${i + 1}. ${status} ${r.method} ${r.fullUrl || r.url} (${duration})`,
      );
    });
  }

  if (failedRequests.length > 0) {
    log("\n--- Failed Requests ---\n");
    failedRequests.forEach((r, i) => {
      log(`  ${i + 1}. [${r.status}] ${r.method} ${r.fullUrl || r.url}`);
      if (r.error) {
        log(`      Error: ${r.error}`);
      }
    });
  }

  log("\n=== END NETWORK SUMMARY ===\n");

  fs.writeFileSync(
    networkLogFile,
    JSON.stringify(
      {
        timestamp: new Date().toISOString(),
        summary,
        apiRequests: apiRequests.map((r) => ({
          method: r.method,
          url: r.fullUrl || r.url,
          status: r.status,
          duration: r.duration,
          resourceType: r.resourceType,
        })),
        failedRequests: failedRequests.map((r) => ({
          method: r.method,
          url: r.fullUrl || r.url,
          status: r.status,
          error: r.error,
        })),
      },
      null,
      2,
    ),
  );

  log(`Network logs saved to: ${networkLogFile}`);

  await closeBrowser(browser);
  log("Done!");
  log(`\nFull log saved to: ${logFile}`);
}

main().catch((err) => {
  log(`Error: ${err.message}`);
  console.error("Error:", err);
  process.exit(1);
});
