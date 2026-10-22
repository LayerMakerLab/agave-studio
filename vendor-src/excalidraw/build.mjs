// Builds web/vendor/excalidraw/ from the packages pinned in package-lock.json. Run it with build.sh.
//
// Out: excalidraw.js (one ES module: Excalidraw, React and ReactDOM), excalidraw.css, fonts/ (every font the editor
// loads), LICENSES.txt (the licence of every package in the bundle and of every font) and SHA256SUMS.
// The build fails when a bundled package has a licence outside MIT, Apache-2.0, BSD and ISC, when the bundle still
// names a CDN, or when Agave's fractional-indexing differs from the original package.

import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";

const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.resolve(here, "../../web/vendor/excalidraw");
const pkgDir = path.join(here, "node_modules/@excalidraw/excalidraw");
const dist = path.join(pkgDir, "dist/prod");
const shim = (name) => path.join(here, "shims", name);
// Where the studio serves the files, and so where Excalidraw loads its fonts from.
const ASSET_PATH = "/vendor/excalidraw/";

const ALLOWED = new Set(["MIT", "ISC", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "0BSD"]);

// Fonts the bundle ships, with their licences (read from each font's own name table and its project).
const FONTS = {
  Assistant: "SIL Open Font License 1.1. Copyright 2020 The Assistant Project Authors (https://github.com/hafontia/Assistant). Copyright 2010 The Source Sans Pro Authors (https://github.com/adobe-fonts/source-sans-pro), with Reserved Font Name 'Source'.",
  Cascadia: "SIL Open Font License 1.1 (https://github.com/microsoft/cascadia-code). Copyright 2020 Microsoft Corporation.",
  ComicShanns: "MIT License. Copyright (c) 2018 Shannon Miwa, (c) 2023 Jesus Gonzalez, (c) 2023 Rodrigo Batista de Moraes, (c) 2024 Fini Jastrow, (c) 2024 Kyle Beechly.",
  Excalifont: "SIL Open Font License 1.1. Copyright (c) 2024 by Excalidraw.",
  Lilita: "SIL Open Font License 1.1. Copyright (c) 2011 Juan Montoreano (juan@remolacha.biz), with Reserved Font Names \"Lilita One\".",
  Nunito: "SIL Open Font License 1.1. Copyright 2014 The Nunito Project Authors (https://github.com/googlefonts/nunito).",
  Virgil: "SIL Open Font License 1.1. Copyright (c) 2011 by Your Own Font Foundry.",
  Xiaolai: "SIL Open Font License 1.1. Copyright (c) 2020 LXGW (https://github.com/lxgw/kose-font).",
};
// Liberation Sans 1.05 (GPL-2.0 with a font exception) is left out; Excalidraw only uses it for server-side
// rendering, and the board treats it as a system font instead.
const SKIPPED_FONTS = ["Liberation"];

function fail(msg) {
  console.error("build failed: " + msg);
  process.exit(1);
}

// Replaces `from` once in `text`; the build fails when it is not there exactly once (a new Excalidraw changed it).
function replaceOnce(text, from, to, what) {
  const n = typeof from === "string" ? text.split(from).length - 1 : (text.match(new RegExp(from.source, "g")) || []).length;
  if (n !== 1) fail(`expected one ${what}, found ${n}`);
  return text.replace(from, to);
}

const agave = {
  name: "agave",
  setup(build) {
    build.onResolve({ filter: /^@excalidraw\/mermaid-to-excalidraw$/ }, () => ({ path: shim("mermaid-to-excalidraw.js") }));
    build.onResolve({ filter: /^pako$/ }, () => ({ path: shim("pako.js") }));
    build.onResolve({ filter: /^fractional-indexing$/ }, () => ({ path: shim("fractional-indexing.js") }));
    build.onResolve({ filter: /^\.\/subset-worker\.chunk\.js$/ }, () => ({ path: shim("subset-worker.js") }));
    // English only: every other translation becomes an empty module.
    build.onResolve({ filter: /^\.\/locales\// }, (args) => {
      const name = path.basename(args.path);
      if (name.startsWith("en-") || name.startsWith("percentages-")) return undefined;
      return { path: shim("empty-locale.js") };
    });
    // The stylesheet's fonts stay files next to it (copied below).
    build.onResolve({ filter: /\.woff2$/ }, (args) => ({ path: args.path, external: true }));
    // Excalidraw falls back to a CDN for a font it cannot load from the asset path: point that at the studio too (an
    // absolute URL, since Excalidraw resolves font paths against it).
    build.onLoad({ filter: /[\\/]@excalidraw[\\/]excalidraw[\\/]dist[\\/]prod[\\/][^\\/]+\.js$/ }, (args) => {
      let text = fs.readFileSync(args.path, "utf8");
      if (text.includes("ASSETS_FALLBACK_URL\",`https://esm.sh/")) {
        text = replaceOnce(text, /ASSETS_FALLBACK_URL",`https:\/\/esm\.sh\/\$\{[^`]*`[^`]*`[^}]*\}\/dist\/prod\/`/, `ASSETS_FALLBACK_URL",(typeof window<"u"?window.location.origin:"")+"${ASSET_PATH}"`, "font CDN fallback");
      }
      // Excalidraw's live-collaboration config holds excalidraw.com's public Firebase web key. The board never
      // collaborates, and the repository's secret scan (gitleaks) refuses any key, so it goes.
      if (text.includes("VITE_APP_FIREBASE_CONFIG:'")) {
        text = replaceOnce(text, /VITE_APP_FIREBASE_CONFIG:'[^']*'/, "VITE_APP_FIREBASE_CONFIG:'{}'", "Firebase config");
      }
      if (text.includes("./fonts/Liberation/LiberationSans-Regular.woff2")) {
        text = replaceOnce(text, "\"./fonts/Liberation/LiberationSans-Regular.woff2\"", "\"local:Liberation Sans\"", "Liberation font path");
      }
      return { contents: text, loader: "js" };
    });
  },
};

async function checkFractionalIndexing() {
  const ours = await import(shim("fractional-indexing.js"));
  const theirs = await import(path.join(here, "node_modules/fractional-indexing/src/index.js"));
  let seed = 1;
  const rand = () => (seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648;
  const keys = [null];
  let k = null;
  for (let i = 0; i < 400; i++) keys.push((k = theirs.generateKeyBetween(k, null)));
  k = keys[1];
  for (let i = 0; i < 400; i++) keys.push((k = theirs.generateKeyBetween(null, k)));
  for (let i = 0; i < 2000; i++) {
    const a = keys[1 + Math.floor(rand() * (keys.length - 1))];
    const b = keys[1 + Math.floor(rand() * (keys.length - 1))];
    if (a !== b) keys.push(theirs.generateKeyBetween(a < b ? a : b, a < b ? b : a));
  }
  keys.push("Zz", "a0", "zzzzzzzzzzzzzzzzzzzzzzzzzzz", "A00000000000000000000000001");
  const sorted = [...new Set(keys.filter((x) => x !== null))].sort();
  let checked = 0;
  const same = (f) => {
    let want, got;
    try { want = JSON.stringify(f(theirs)); } catch (e) { want = "throws"; }
    try { got = JSON.stringify(f(ours)); } catch (e) { got = "throws"; }
    if (want !== got) fail(`fractional-indexing differs: ${f.toString()} gave ${got}, want ${want}`);
    checked++;
  };
  for (let i = 0; i < 5000; i++) {
    const a = rand() < 0.1 ? null : sorted[Math.floor(rand() * sorted.length)];
    const b = rand() < 0.1 ? null : sorted[Math.floor(rand() * sorted.length)];
    const n = Math.floor(rand() * 6);
    same((m) => m.generateKeyBetween(a, b));
    same((m) => m.generateNKeysBetween(a, b, n));
  }
  for (const bad of ["", "a", "a00", "b0", "!", "a0" + "0"]) same((m) => m.generateKeyBetween(bad, null));
  return checked;
}

function packageOf(input) {
  const parts = input.split("node_modules/");
  if (parts.length < 2) return null;
  const rest = parts[parts.length - 1].split("/");
  const name = rest[0].startsWith("@") ? rest[0] + "/" + rest[1] : rest[0];
  return path.join(parts.slice(0, -1).join("node_modules/"), "node_modules", name);
}

function licenceText(dir) {
  const file = fs.readdirSync(dir).find((f) => /^(licen[cs]e|copying)(-mit)?(\.md|\.txt)?$/i.test(f));
  return file ? fs.readFileSync(path.join(dir, file), "utf8").trim() : null;
}

function sha256(file) {
  return createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function walk(dir, base = dir) {
  return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = path.join(dir, e.name);
    return e.isDirectory() ? walk(p, base) : [path.relative(base, p)];
  });
}

const fiChecks = await checkFractionalIndexing();

fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(out, { recursive: true });

const result = await esbuild.build({
  absWorkingDir: here,
  entryPoints: { excalidraw: "entry.js" },
  bundle: true,
  format: "esm",
  platform: "browser",
  target: "es2020",
  minify: true,
  conditions: ["production"],
  define: { "process.env.NODE_ENV": "\"production\"", "process.env.IS_PREACT": "\"false\"" },
  legalComments: "none",
  outdir: out,
  metafile: true,
  logLevel: "warning",
  plugins: [agave],
});
if (result.errors.length) fail("esbuild errors");

// The licence of every package in the bundle.
const pkgs = new Map();
for (const input of Object.keys(result.metafile.inputs)) {
  const dir = packageOf(input);
  if (!dir || pkgs.has(dir)) continue;
  const pj = JSON.parse(fs.readFileSync(path.join(here, dir, "package.json"), "utf8"));
  pkgs.set(dir, pj);
}
// Packages that ship no licence file of their own: the text from their upstream repository, kept in licenses/.
const UPSTREAM = { "@excalidraw/excalidraw": "excalidraw.txt", "react-remove-scroll-bar": "react-remove-scroll-bar.txt" };
const upstream = (name) => UPSTREAM[name] || (name.startsWith("@radix-ui/") ? "radix-ui-primitives.txt" : null);
const notices = new Map();
for (const [dir, pj] of [...pkgs].sort((a, b) => a[1].name.localeCompare(b[1].name) || a[1].version.localeCompare(b[1].version))) {
  // Old packages list their licences as an array of {type}.
  const lic = typeof pj.license === "string" ? pj.license : Array.isArray(pj.licenses) && pj.licenses.length === 1 ? pj.licenses[0].type : "";
  if (!ALLOWED.has(lic)) fail(`${pj.name}@${pj.version} is licensed "${lic}", outside MIT, Apache-2.0, BSD and ISC`);
  let text = licenceText(path.join(here, dir));
  if (!text && upstream(pj.name)) text = fs.readFileSync(path.join(here, "licenses", upstream(pj.name)), "utf8").trim();
  if (!text) fail(`${pj.name}@${pj.version} has no licence file`);
  notices.set(`${pj.name} ${pj.version}`, `${pj.name} ${pj.version} (${lic})\n\n${text}`);
}

// Fonts: every family Excalidraw loads, except the ones left out above.
const fontSrc = path.join(dist, "fonts");
const families = fs.readdirSync(fontSrc).filter((d) => !SKIPPED_FONTS.includes(d));
for (const fam of families) {
  if (!FONTS[fam]) fail(`no licence recorded for the font family ${fam}`);
  fs.cpSync(path.join(fontSrc, fam), path.join(out, "fonts", fam), { recursive: true });
}
const fontNotes = families.map((f) => `- ${f} (fonts/${f}/): ${FONTS[f]}`).join("\n");
fs.writeFileSync(
  path.join(out, "LICENSES.txt"),
  "Third-party notices for web/vendor/excalidraw/ (built by vendor-src/excalidraw/build.sh).\n\n" +
    "Fonts (the SIL Open Font License 1.1 is in web/assets/fonts/OFL-Geist.txt; each font keeps its own copyright):\n" +
    fontNotes + "\n\n" +
    "Packages in excalidraw.js and excalidraw.css:\n\n" +
    [...notices.values()].join("\n\n" + "-".repeat(78) + "\n\n") + "\n\n" + "-".repeat(78) + "\n\n" +
    "Compiled into @excalidraw/excalidraw's font subsetting (WebAssembly): HarfBuzz and Google's woff2.\n\n" +
    ["harfbuzz.txt", "woff2.txt"].map((f) => fs.readFileSync(path.join(here, "licenses", f), "utf8").trim()).join("\n\n" + "-".repeat(78) + "\n\n") + "\n",
);

// No CDN may be left in the code: the page may only fetch from the studio (connect-src 'self').
const js = fs.readFileSync(path.join(out, "excalidraw.js"), "utf8");
for (const cdn of ["esm.sh", "unpkg.com", "jsdelivr.net", "cdnjs"]) {
  if (js.includes(cdn)) fail(`the bundle still names ${cdn}`);
}
// Nor any key: a Google API key, or an apiKey with a value.
if (/AIza[0-9A-Za-z_-]{35}/.test(js) || /apiKey["']?\s*:\s*["'][^"']+["']/.test(js)) fail("the bundle still holds an API key");

const files = walk(out).filter((f) => f !== "SHA256SUMS").sort();
const sums = files.map((f) => `${sha256(path.join(out, f))}  ${f}`).join("\n") + "\n";
fs.writeFileSync(path.join(out, "SHA256SUMS"), sums);

const size = (f) => fs.statSync(path.join(out, f)).size;
const total = files.reduce((s, f) => s + size(f), 0);
console.log(`fractional-indexing: ${fiChecks} comparisons with the original, all equal`);
console.log(`packages in the bundle: ${notices.size}, all MIT, Apache-2.0, BSD or ISC`);
console.log(`excalidraw.js ${size("excalidraw.js")} bytes, excalidraw.css ${size("excalidraw.css")} bytes`);
console.log(`${files.length} files, ${total} bytes in ${path.relative(path.resolve(here, "../.."), out)}/`);
