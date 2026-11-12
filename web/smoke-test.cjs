// Smoke test of the browser build: opens the page in headless Chromium (WebGPU on SwiftShader, so no GPU is needed),
// starts a model, places a few voxels and switches through Render and Effects mode. It fails on any page error, any
// console error (the GPU's shader compiler reports there: browsers check WGSL more strictly than naga does) and on a
// canvas that never got its size.
//
//   node web/smoke-test.cjs http://localhost:8000/ [screenshot-dir]
//
// Needs the `playwright` package and its Chromium (`npx playwright install chromium`).
"use strict";

const { chromium } = require("playwright");

const WEBGPU_ARGS = [
  "--enable-unsafe-webgpu",
  "--enable-features=Vulkan,WebGPU",
  "--use-vulkan=swiftshader",
  "--use-webgpu-adapter=swiftshader",
  "--use-angle=swiftshader",
  "--ignore-gpu-blocklist",
];

// Points in a 1440 × 900 viewport.
const NEW_MODEL = [797, 386];
const VOXELS = [[700, 450], [740, 450], [780, 450]];
const RENDER_TAB = [281, 35];
const EFFECTS_TAB = [226, 35];

async function main() {
  const [url, screenshots] = process.argv.slice(2);
  if (!url) {
    throw new Error("usage: node web/smoke-test.cjs URL [screenshot-dir]");
  }
  const problems = [];
  const browser = await chromium.launch({ args: WEBGPU_ARGS });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    page.on("pageerror", (error) => problems.push(`page error: ${error.message}`));
    page.on("console", (message) => {
      const text = message.text();
      console.log(`[console.${message.type()}] ${text.split("\n")[0]}`);
      if (message.type() === "error") {
        problems.push(`console error: ${text}`);
      }
    });

    const ready = page
      .waitForEvent("console", { predicate: (message) => message.text().includes("viewport ready"), timeout: 120_000 })
      .catch(() => problems.push("the editor never reported that its viewport is ready"));
    await page.goto(url);
    const adapter = await page.evaluate(async () => Boolean(navigator.gpu && (await navigator.gpu.requestAdapter())));
    if (!adapter) {
      throw new Error("this Chromium offers no WebGPU adapter, so the build cannot be tested here");
    }
    await ready;

    const step = async (name, action) => {
      await action();
      await page.waitForTimeout(4000);
      if (screenshots) {
        await page.screenshot({ path: `${screenshots}/${name}.png` });
      }
    };
    await step("model", async () => {
      await page.mouse.click(...NEW_MODEL);
      await page.waitForTimeout(1000);
      for (const point of VOXELS) {
        await page.mouse.click(...point);
        await page.waitForTimeout(300);
      }
    });
    await step("render", () => page.mouse.click(...RENDER_TAB));
    await step("effects", () => page.mouse.click(...EFFECTS_TAB));

    const canvas = await page.evaluate(() => {
      const element = document.getElementById("rustium");
      return { width: element.width, height: element.height };
    });
    if (canvas.width < 1000 || canvas.height < 600) {
      problems.push(`the canvas draws at ${canvas.width} × ${canvas.height} pixels`);
    }
    const fatal = await page.evaluate(() => document.getElementById("rustium-error").textContent);
    if (fatal) {
      problems.push(`the page shows an error: ${fatal}`);
    }
  } finally {
    await browser.close();
  }

  if (problems.length > 0) {
    console.error(`\nSmoke test failed:\n- ${problems.join("\n- ")}`);
    process.exit(1);
  }
  console.log("\nSmoke test passed.");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
