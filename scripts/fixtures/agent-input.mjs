import { writeFileSync } from "node:fs";

process.stdin.setRawMode(true);
process.stdin.resume();
process.stdout.write("\x1b[2J\x1b[H\x1b[?2004h> ");
let received = "";
process.stdin.on("data", (data) => {
  received += data.toString();
  writeFileSync(process.argv[2] ?? process.env.P2PCLI_INPUT_CAPTURE, JSON.stringify(received));
});
