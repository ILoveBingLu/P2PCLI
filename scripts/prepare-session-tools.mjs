// Compatibility entry point: preparation is now local validation only.
import { main } from "./check-session-tools.mjs";
main().catch((error) => { console.error(error.message); process.exitCode = 1; });
