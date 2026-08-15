// Usage: node mammoth_convert.js <input> <output.md> [iters]
// Writes markdown to <output.md>, prints "min_ms\tmean_ms" to stdout.
const fs = require("fs");
const mammoth = require("mammoth");
const TurndownService = require("turndown");
const { gfm } = require("turndown-plugin-gfm");

(async () => {
  const [file, out, itersArg] = process.argv.slice(2);
  if (!file || !out) {
    console.error("usage: node mammoth_convert.js <input> <output.md> [iters]");
    process.exit(2);
  }
  const iters = Math.max(1, parseInt(itersArg || "1", 10));
  const td = new TurndownService({ headingStyle: "atx", codeBlockStyle: "fenced" });
  td.use(gfm);
  td.addRule("dropDataImages", {
    filter: (node) => node.nodeName === "IMG" && /^data:/.test(node.getAttribute("src") || ""),
    replacement: () => "",
  });

  let md = "";
  const times = [];
  for (let i = 0; i < iters; i++) {
    const t0 = process.hrtime.bigint();
    const res = await mammoth.convertToHtml({ path: file });
    md = td.turndown(res.value);
    times.push(Number(process.hrtime.bigint() - t0) / 1e6);
  }
  fs.writeFileSync(out, md, "utf8");
  const mean = times.reduce((a, b) => a + b, 0) / times.length;
  console.log(`${Math.min(...times).toFixed(2)}\t${mean.toFixed(2)}`);
})().catch((e) => {
  console.error(String(e && e.message ? e.message : e));
  process.exit(1);
});
