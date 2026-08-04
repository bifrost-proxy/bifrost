const assert = require("node:assert/strict");
const test = require("node:test");
const {
  buildChildEnvironment,
  detectPackageManager,
} = require("./install-source.js");

test("detects pnpm global and content-addressed layouts", () => {
  assert.equal(
    detectPackageManager(
      "/Users/test/Library/pnpm/global/5/.pnpm/@bifrost-proxy+bifrost@1.2.3/node_modules/@bifrost-proxy/bifrost/bin/bifrost"
    ),
    "pnpm"
  );
  assert.equal(
    detectPackageManager(
      "C:\\Users\\test\\AppData\\Local\\pnpm\\global\\5\\node_modules\\@bifrost-proxy\\bifrost\\bin\\bifrost"
    ),
    "pnpm"
  );
});

test("defaults npm package layouts to npm", () => {
  assert.equal(
    detectPackageManager(
      "/usr/local/lib/node_modules/@bifrost-proxy/bifrost/bin/bifrost"
    ),
    "npm"
  );
});

test("builds a child environment that overrides stale source hints", () => {
  assert.deepEqual(
    buildChildEnvironment(
      "/tmp/.pnpm/node_modules/@bifrost-proxy/bifrost/bin/bifrost",
      { KEEP_ME: "yes", BIFROST_CLI_INSTALL_SOURCE: "npm" }
    ),
    { KEEP_ME: "yes", BIFROST_CLI_INSTALL_SOURCE: "pnpm" }
  );
});
