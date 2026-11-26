import { createRequire } from "node:module";

process.env.NEXT_TELEMETRY_DISABLED = "1";
const require = createRequire(import.meta.url);
const next = require.resolve("next/dist/bin/next");
process.argv[1] = next;
await import(require("node:url").pathToFileURL(next).href);
