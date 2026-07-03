#!/usr/bin/env node

import { collectHomeErrors } from "./home-static-lib.mjs";

const errors = await collectHomeErrors();
if (errors.length > 0) {
  console.error("Static home verification failed:");
  for (const error of errors) {
    console.error(`- ${error}`);
  }
  process.exit(1);
}

console.log("Static home verification passed.");
