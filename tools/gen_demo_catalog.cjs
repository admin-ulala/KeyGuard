// 用真实 catalog 生成界面演示数据，避免手抄漂移
const fs = require("fs");
const { execFileSync } = require("child_process");

const exe = "target/release/kg-agent.exe";
const json = execFileSync(exe, ["catalog-json"], { encoding: "utf8" }).trim();
const parsed = JSON.parse(json);
const n = (parsed.entries || []).length;
if (n < 20) throw new Error("目录条目太少: " + n);

const p = "crates/kg-ui/assets/index.html";
let s = fs.readFileSync(p, "utf8");
if (!s.includes("const DEMO_CAT")) throw new Error("index.html 里没有 DEMO_CAT 占位");

s = s.replace(
  /const DEMO_CAT = [\s\S]*?;\n/,
  "const DEMO_CAT = " + JSON.stringify(parsed) + ";\n",
);
fs.writeFileSync(p, s);
console.log("DEMO_CAT entries=" + n + " bytes=" + json.length);
