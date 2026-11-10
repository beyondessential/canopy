// Renders mockups to full-page PNGs at 1440px and reports overflow and wrapped chips.
//   node .workhorse/plans/p6/design/build/render.mjs <out-dir> <file.html>...
import { createRequire } from "node:module";
import path from "node:path";
const require = createRequire(path.resolve("private-web/package.json"));
const { chromium } = require("playwright");
const [out, ...files] = process.argv.slice(2);
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
for (const f of files) {
  await page.goto("file://" + path.resolve(f));
  const issues = await page.evaluate(() => {
    const r = [];
    if (document.documentElement.scrollWidth > window.innerWidth) r.push("page scrolls horizontally");
    for (const c of document.querySelectorAll(".chip"))
      if (c.getBoundingClientRect().height > 30) r.push("wrapped chip: " + c.textContent.trim());
    for (const e of document.querySelectorAll("td,th,.box"))
      if (e.scrollWidth > e.clientWidth + 1) r.push("overflow: " + e.textContent.trim().slice(0, 40));
    return r;
  });
  const name = path.basename(f, ".html");
  await page.screenshot({ path: path.join(out, name + ".png"), fullPage: true });
  console.log(name, issues.length ? issues.join("; ") : "ok");
}
await browser.close();
