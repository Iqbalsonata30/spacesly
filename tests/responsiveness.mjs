/**
 * Responsiveness verification for the OCP connector settings page.
 *
 * Boots a local vite dev server, renders the OCP settings harness in headless
 * Chromium (via playwright-core), and asserts that the layout stays usable
 * across a range of viewport widths: no horizontal overflow, mode cards stack
 * below 760px, the header health badge collapses below 600px, and the
 * connection-testing stepper renders without overflow.
 *
 * The Tauri IPC backend is stubbed via window.__TAURI_INTERNALS__.invoke so the
 * component renders in a plain browser (the harness page is src/routes/__resp).
 *
 * Usage:
 *   bun tests/responsiveness.mjs
 *
 * Env:
 *   SPACESLY_VITE_PORT  dev server port (default 5199)
 *   SPACESLY_NSS_LIB    directory containing libnss3.so/libnspr4.so
 *                       (auto-extracted from Ubuntu .debs when unset)
 */
import { execSync, spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { chromium } from "playwright-core";

const ROOT = dirname(import.meta.dirname ?? process.cwd());
const PORT = Number(process.env.SPACESLY_VITE_PORT ?? 5199);
const URL = `http://127.0.0.1:${PORT}/__resp`;
const URL_MCP = `http://127.0.0.1:${PORT}/__resp-mcp`;
const ARTIFACTS = join(ROOT, "tests", "responsiveness-artifacts");
const VIEWPORTS = [1280, 1024, 900, 800, 760, 700, 640, 560, 480, 400, 360];
const MODE_STACK_BREAKPOINT = 760;
const BADGE_COLLAPSE_BREAKPOINT = 600;

// ── Tauri IPC mock payloads ──────────────────────────────────────────────────

function mockStatus() {
  const now = Date.now();
  const cfg = {
    version: 1,
    mode: "kubeconfig",
    kubeconfig_path: "/home/user/.kube/config",
    kubeconfig_context: "prod",
    server: null,
    ca_data_set: true,
    token_set: false,
    default_namespace: "default",
    display_name: "Production OpenShift",
    environment_label: "production",
    timeout_policy: {},
    preflight_passed: true,
    updated_at_ms: now - 120_000,
    checksum: "abc123",
  };
  return {
    config: cfg,
    last_known_good: cfg,
    breaker_state: "closed",
    audit: [
      {
        timestamp: new Date(now - 120_000).toISOString(),
        event: "Preflight passed",
        tool: null,
        target: null,
        outcome: "passed",
        detail: null,
        latency_ms: 600,
      },
    ],
  };
}

function mockPreflight() {
  const mk = (stage, name, passed, detail, error_code = null, duration_ms = 5) => ({
    stage,
    name,
    required: true,
    passed,
    detail,
    duration_ms,
    error_code,
  });
  return {
    passed: false,
    passed_with_warnings: false,
    failed_required: 2,
    total_duration_ms: 612,
    checks: [
      mk("environment", "Validating configuration", true, "Config is well-formed"),
      mk("config", "Verifying URL and certificates", true, "TLS handshake OK"),
      mk("dns_probe", "Resolving hostname", true, "api.cluster.example → 10.0.0.5"),
      mk("connectivity", "Connecting to cluster", true, "TCP connect OK"),
      mk(
        "auth",
        "Verifying identity",
        false,
        "401 Unauthorized — token invalid or expired",
        "AUTH_UNAUTHORIZED",
        400,
      ),
      mk("rbac", "Checking permissions", false, "Skipped: authentication failed"),
    ],
  };
}

// ── NSS libraries (libnss3/libnspr4 for Chromium) ───────────────────────────

function resolveNssLib() {
  const candidates = [
    process.env.SPACESLY_NSS_LIB,
    "/usr/lib/x86_64-linux-gnu",
    "/tmp/opencode/nss-deb/extracted/usr/lib/x86_64-linux-gnu",
    join(tmpdir(), "spacesly-nss", "usr", "lib", "x86_64-linux-gnu"),
  ].filter(Boolean);

  for (const dir of candidates) {
    if (existsSync(join(dir, "libnss3.so")) && existsSync(join(dir, "libnspr4.so"))) {
      return dir;
    }
  }

  // Last resort: download + extract the Ubuntu packages (no sudo needed).
  const work = join(tmpdir(), "spacesly-nss");
  if (!existsSync(work)) {
    mkdirSync(work, { recursive: true });
    execSync(`apt download libnspr4 libnss3 && for f in *.deb; do dpkg-deb -x "$f" .; done`, {
      cwd: work,
      stdio: "inherit",
    });
  }
  const dir = join(work, "usr", "lib", "x86_64-linux-gnu");
  if (!existsSync(join(dir, "libnss3.so"))) {
    throw new Error(
      "Could not find libnss3.so/libnspr4.so. Set SPACESLY_NSS_LIB to a directory containing them.",
    );
  }
  return dir;
}

// ── Vite dev server ──────────────────────────────────────────────────────────

async function waitForServer(url, timeoutMs = 120_000) {
  const deadline = Date.now() + timeoutMs;
  let last = null;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url);
      if (res.status < 500) return res.status;
      last = res.status;
    } catch (error) {
      last = error.code ?? String(error);
    }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`vite dev server did not become ready at ${url} (last: ${last})`);
}

async function isServerUp(url) {
  try {
    const res = await fetch(url);
    return res.status < 500;
  } catch {
    return false;
  }
}

function startServer() {
  const child = spawn("./node_modules/.bin/vite", ["dev", "--port", String(PORT), "--strictPort"], {
    cwd: ROOT,
    stdio: "ignore",
    detached: true,
  });
  child.unref();
  return child;
}

function stopServer(child) {
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    /* already gone */
  }
}

// ── Measurements ─────────────────────────────────────────────────────────────

async function measureLayout(page) {
  return page.evaluate(() => {
    const round = (v) => Math.round(v * 100) / 100;
    const tops = [...document.querySelectorAll(".mode-card")].map((el) =>
      Math.round(el.getBoundingClientRect().top),
    );
    const modeRows = new Set(tops).size;

    const last = document.querySelector(".health-last");
    const buttons = [...document.querySelectorAll(".test-actions button")].map((el) => {
      const r = el.getBoundingClientRect();
      return { left: round(r.left), right: round(r.right), top: round(r.top) };
    });

    const wideElements = [...document.querySelectorAll(".settings-card, .settings-row")].map((el) =>
      round(el.getBoundingClientRect().right),
    );

    return {
      innerWidth: window.innerWidth,
      docScrollWidth: document.documentElement.scrollWidth,
      overflow: document.documentElement.scrollWidth - window.innerWidth,
      maxElementRight: wideElements.length ? Math.max(...wideElements) : 0,
      modeRows,
      modeCardTops: tops,
      healthBadgePresent: !!document.querySelector(".health-badge"),
      healthLastVisible: last ? getComputedStyle(last).display !== "none" : null,
      testButtons: buttons,
      settingsPageWidth: round(
        document.querySelector(".settings-page")?.getBoundingClientRect().width ?? 0,
      ),
      sectionTitles: [...document.querySelectorAll(".settings-section-block h4")].map((el) =>
        el.textContent?.trim(),
      ),
    };
  });
}

async function runInteractionScenario(page, viewportWidth, results) {
  const label = `interaction@${viewportWidth}px`;
  await page.locator('input[value="api_server_token"]').check();
  await page.locator("#ocp-server").fill("https://api.cluster.example:6443");
  await page.locator("#ocp-token").fill("mock-token");
  await page.locator(".btn-primary").click();
  await page.waitForSelector(".preflight-stepper", { timeout: 30_000 });
  await page.waitForSelector(".preflight-step-fail", { timeout: 30_000 });
  await page.waitForTimeout(400);

  const m = await measureLayout(page);
  const stepCount = await page.locator(".preflight-step").count();
  const failCount = await page.locator(".preflight-step-fail").count();
  const badgeFail = await page.locator(".step-badge-fail").count();

  const failures = [];
  if (m.overflow > 1) failures.push(`horizontal overflow ${m.overflow}px`);
  if (stepCount < 5) failures.push(`expected ≥5 preflight steps, got ${stepCount}`);
  if (failCount < 1) failures.push("expected ≥1 failed step highlighted");
  if (badgeFail < 1) failures.push("expected failed-step badge");
  if (m.maxElementRight > m.innerWidth + 1)
    failures.push(`element overflows viewport: right ${m.maxElementRight} > ${m.innerWidth}`);

  await page.screenshot({
    path: join(ARTIFACTS, `scenario-${viewportWidth}.png`),
    fullPage: true,
  });

  results.push({
    viewport: label,
    status: failures.length === 0 ? "PASS" : "FAIL",
    details: failures.length
      ? failures.join("; ")
      : `stepper ok (${stepCount} steps, ${failCount} failed)`,
  });
}

// ── McpConnectionSettings verification ───────────────────────────────────────

async function measureMcpLayout(page) {
  return page.evaluate(() => {
    const round = (v) => Math.round(v * 100) / 100;
    const wideElements = [...document.querySelectorAll(".settings-card, .settings-row")].map((el) =>
      round(el.getBoundingClientRect().right),
    );
    const rows = [...document.querySelectorAll(".settings-row")].map((el) => {
      const labels = el.querySelectorAll(".settings-label");
      const tops = [...labels].map((n) => Math.round(n.getBoundingClientRect().top));
      return { inputs: labels.length, rows: new Set(tops).size };
    });

    return {
      innerWidth: window.innerWidth,
      docScrollWidth: document.documentElement.scrollWidth,
      overflow: document.documentElement.scrollWidth - window.innerWidth,
      maxElementRight: wideElements.length ? Math.max(...wideElements) : 0,
      tagEditorCount: document.querySelectorAll(".tag-editor").length,
      chipCount: document.querySelectorAll(".tag-editor .chip").length,
      argumentEditorCount: document.querySelectorAll(".argument-editor").length,
      argumentRowCount: document.querySelectorAll(
        ".argument-editor .argument-row:not(.argument-row-new)",
      ).length,
      commandPreview: document.querySelector(".command-preview-line")?.textContent?.trim() ?? "",
      rows,
      settingsPageWidth: round(
        document.querySelector(".settings-page")?.getBoundingClientRect().width ?? 0,
      ),
    };
  });
}

async function runMcpScenario(page, viewportWidth, results) {
  const label = `mcp@${viewportWidth}px`;

  // Add a tag to Agent Domains by typing + Enter
  const domainsInput = page.locator("#mcp-domains");
  await domainsInput.fill("ansible");
  await domainsInput.press("Enter");

  // Add an argument that contains a comma (must NOT split) and keep the add input focused.
  const argsInput = page.locator("#mcp-args");
  await argsInput.click();
  await argsInput.fill("--filter=env,prod");
  await argsInput.press("Enter");

  const addInputFocused = await argsInput.evaluate((element) => document.activeElement === element);

  // Existing arguments are directly editable and preserve their positions.
  await page.locator("#mcp-args-row-0").fill("--transport=stdio");
  await page.locator(".argument-remove").nth(1).click();

  await page.waitForTimeout(150);
  const m = await measureMcpLayout(page);

  const domains = await page.locator(".tag-editor:has(#mcp-domains) .chip").allTextContents();
  const hasAnsible = domains.some((t) => t.includes("ansible"));
  const args = await page
    .locator(".argument-row .argument-input")
    .evaluateAll((inputs) => inputs.map((input) => input.value));
  const commaArgPreserved = args.includes("--filter=env,prod");
  const argumentOrderPreserved = args[0] === "--transport=stdio" && args[1] === "--filter=env,prod";
  const preview = await page.locator(".command-preview-line").textContent();
  const previewUpdated =
    preview?.replace(/\s+/g, " ").trim() === "$ uvx --transport=stdio --filter=env,prod";

  // Remove a chip via its × button
  const firstRemove = page.locator(".tag-editor-chips .chip-remove").first();
  const before = await page.locator(".tag-editor .chip").count();
  await firstRemove.click();
  await page.waitForTimeout(100);
  const after = await page.locator(".tag-editor .chip").count();

  const failures = [];
  if (m.overflow > 1) failures.push(`horizontal overflow ${m.overflow}px`);
  if (m.maxElementRight > m.innerWidth + 1)
    failures.push(`element overflows viewport: right ${m.maxElementRight} > ${m.innerWidth}`);
  if (m.tagEditorCount < 2) failures.push(`expected ≥2 tag editors, got ${m.tagEditorCount}`);
  if (m.argumentEditorCount !== 1)
    failures.push(`expected 1 argument editor, got ${m.argumentEditorCount}`);
  if (!hasAnsible) failures.push("Agent Domains tag not added via Enter");
  if (!commaArgPreserved) failures.push("argument with comma was split");
  if (!argumentOrderPreserved) failures.push(`argument order/edit failed: ${args.join(" | ")}`);
  if (!previewUpdated) failures.push(`command preview did not update: ${preview}`);
  if (!addInputFocused) failures.push("add argument input did not retain focus after Enter");
  if (after !== before - 1) failures.push(`chip removal failed: ${before} -> ${after}`);

  await page.screenshot({
    path: join(ARTIFACTS, `mcp-${viewportWidth}.png`),
    fullPage: true,
  });

  results.push({
    viewport: label,
    status: failures.length === 0 ? "PASS" : "FAIL",
    details: failures.length
      ? failures.join("; ")
      : `argument editor and preview ok · tag chips ${after}/${before}`,
  });
}

// ── Main ─────────────────────────────────────────────────────────────────────

const results = [];
const serverAlreadyUp = await isServerUp(URL);
const server = serverAlreadyUp ? null : startServer();
let browser;
try {
  await waitForServer(URL);

  const nssLib = resolveNssLib();
  browser = await chromium.launch({
    headless: true,
    args: ["--no-sandbox", "--disable-gpu"],
    env: { ...process.env, LD_LIBRARY_PATH: nssLib },
  });

  mkdirSync(ARTIFACTS, { recursive: true });

  for (const width of VIEWPORTS) {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    await context.addInitScript(
      ({ status, preflight }) => {
        window.__TAURI_INTERNALS__ = {
          invoke: async (cmd) => {
            if (cmd === "ocp_connector_status") return status;
            if (cmd === "ocp_secret_status") return { token_set: false, ca_data_set: false };
            if (cmd === "ocp_preflight") return preflight;
            if (cmd === "ocp_save_draft") return { ...status.config, token_set: true };
            throw new Error("mock: unhandled command " + cmd);
          },
          transformCallback: (_cb) => 1,
          convertFileSrc: (s) => s,
        };
      },
      { status: mockStatus(), preflight: mockPreflight() },
    );

    const page = await context.newPage();
    const pageErrors = [];
    page.on("pageerror", (e) => pageErrors.push(e.message));

    await page.goto(URL, { waitUntil: "load", timeout: 180_000 });
    await page.waitForSelector(".settings-page", { timeout: 60_000 });
    await page.waitForTimeout(800);

    const m = await measureLayout(page);
    const expectedRows = width <= MODE_STACK_BREAKPOINT ? 3 : 1;
    const expectedLast = width > BADGE_COLLAPSE_BREAKPOINT;

    const failures = [];
    if (m.overflow > 1) failures.push(`horizontal overflow ${m.overflow}px`);
    if (!m.healthBadgePresent) failures.push("health badge missing");
    if (m.healthLastVisible !== expectedLast)
      failures.push(
        `health "last checked" visible=${m.healthLastVisible}, expected=${expectedLast}`,
      );
    if (m.modeRows !== expectedRows)
      failures.push(`mode-card rows=${m.modeRows}, expected=${expectedRows}`);
    if (m.maxElementRight > m.innerWidth + 1)
      failures.push(`element overflows viewport: right ${m.maxElementRight} > ${m.innerWidth}`);
    for (const b of m.testButtons) {
      if (b.left < -1 || b.right > m.innerWidth + 1)
        failures.push(`test button out of bounds [${b.left}, ${b.right}]`);
    }
    if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 3).join(" | ")}`);

    await page.screenshot({
      path: join(ARTIFACTS, `viewport-${width}.png`),
      fullPage: true,
    });

    results.push({
      viewport: `${width}px`,
      status: failures.length === 0 ? "PASS" : "FAIL",
      details: failures.length
        ? failures.join("; ")
        : `no overflow · rows=${m.modeRows} · last-checked visible=${m.healthLastVisible}`,
    });

    await runInteractionScenario(page, width, results);
    await context.close();
  }

  // ── McpConnectionSettings (generic MCP) across viewports ────────────────────
  for (const width of VIEWPORTS) {
    const context = await browser.newContext({ viewport: { width, height: 900 } });
    await context.addInitScript(() => {
      window.__TAURI_INTERNALS__ = {
        invoke: async () => {
          throw new Error("mock: McpConnectionSettings should not invoke IPC");
        },
        transformCallback: (_cb) => 1,
        convertFileSrc: (s) => s,
      };
    });

    const page = await context.newPage();
    const pageErrors = [];
    page.on("pageerror", (e) => pageErrors.push(e.message));

    await page.goto(URL_MCP, { waitUntil: "load", timeout: 180_000 });
    await page.waitForSelector(".settings-page", { timeout: 60_000 });
    await page.waitForTimeout(500);

    const m = await measureMcpLayout(page);

    const failures = [];
    if (m.overflow > 1) failures.push(`horizontal overflow ${m.overflow}px`);
    if (m.maxElementRight > m.innerWidth + 1)
      failures.push(`element overflows viewport: right ${m.maxElementRight} > ${m.innerWidth}`);
    if (m.tagEditorCount < 2) failures.push(`expected ≥2 tag editors, got ${m.tagEditorCount}`);
    if (m.argumentEditorCount !== 1)
      failures.push(`expected 1 argument editor, got ${m.argumentEditorCount}`);
    if (m.argumentRowCount !== 2)
      failures.push(`expected 2 ordered arguments, got ${m.argumentRowCount}`);
    if (m.commandPreview.replace(/\s+/g, " ") !== "$ uvx --transport stdio")
      failures.push(`unexpected command preview: ${m.commandPreview}`);
    if (m.chipCount < 1) failures.push("no chips rendered from initial values");
    // Rows with >1 field must stack to a single column on narrow viewports
    const wideRow = m.rows.find((r) => r.inputs > 1);
    if (wideRow) {
      // A 2-column row renders as 1 row when side-by-side, or N rows when stacked
      const expectedRows = width <= 640 ? wideRow.inputs : 1;
      if (wideRow.rows !== expectedRows)
        failures.push(
          `settings-row layout: ${wideRow.inputs} fields in ${wideRow.rows} row(s), expected ${expectedRows}`,
        );
    }
    if (pageErrors.length) failures.push(`page errors: ${pageErrors.slice(0, 3).join(" | ")}`);

    await page.screenshot({
      path: join(ARTIFACTS, `mcp-viewport-${width}.png`),
      fullPage: true,
    });

    results.push({
      viewport: `mcp-${width}px`,
      status: failures.length === 0 ? "PASS" : "FAIL",
      details: failures.length
        ? failures.join("; ")
        : `no overflow · ordered arguments ${m.argumentRowCount} · preview ok`,
    });

    await runMcpScenario(page, width, results);
    await context.close();
  }
} finally {
  if (browser) await browser.close().catch(() => {});
  if (server) stopServer(server);
}

// ── Report ───────────────────────────────────────────────────────────────────

const width = Math.max(...results.map((r) => r.viewport.length));
console.log("\nResponsiveness verification");
console.log("=".repeat(width + 14));
let failures = 0;
for (const r of results) {
  const mark = r.status === "PASS" ? "✓" : "✗";
  console.log(`${mark} ${r.viewport.padEnd(width)}  ${r.status}  ${r.details}`);
  if (r.status !== "PASS") failures += 1;
}
console.log("=".repeat(width + 14));
console.log(`Artifacts: ${ARTIFACTS}`);
console.log(failures === 0 ? "ALL CHECKS PASSED" : `${failures} CHECK(S) FAILED`);
process.exit(failures === 0 ? 0 : 1);
