#!/usr/bin/env node

import { buildHome } from "./home-static-lib.mjs";

const result = await buildHome();
console.log(`Static home page built at ${result.htmlPath}`);
