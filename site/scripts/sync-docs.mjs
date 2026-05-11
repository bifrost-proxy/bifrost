import { formatCoverageSummary, syncDocs } from "./docs-sync-lib.mjs";

const pages = await syncDocs();

console.log(`Synced ${pages.length} docs pages:`);
console.log(formatCoverageSummary(pages));
