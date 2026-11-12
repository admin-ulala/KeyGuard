// 前台是本程序自己时，别在 hero 里显示 "kg-ui.exe"，说人话
const fs = require("fs");
const p = "crates/kg-ui/assets/index.html";
let s = fs.readFileSync(p, "utf8");
const must = (c, m) => {
  if (!c) throw new Error(m);
};

must(s.includes(`        <span>\${st.fg.exe ? esc(st.fg.exe) : '当前前台：无'}</span>`), "hero meta");
s = s.replace(
  `        <span>\${st.fg.exe ? esc(st.fg.exe) : '当前前台：无'}</span>`,
  `        <span>\${fgLabel(st.fg)}</span>`,
);

must(s.includes(`        <div class="mrow"><span>前台进程</span><b>\${esc(st.fg.exe||'—')}</b></div>`), "diag row");
s = s.replace(
  `        <div class="mrow"><span>前台进程</span><b>\${esc(st.fg.exe||'—')}</b></div>`,
  `        <div class="mrow"><span>前台进程</span><b>\${fgLabel(st.fg)}</b></div>`,
);

// fgLabel：本程序自己 → 说人话；否则原样
const anchor = "function esc(s){";
must(s.includes(anchor), "esc");
s = s.replace(
  anchor,
  `// 前台是本程序（界面/Agent）时，显示 "kg-ui.exe" 纯属噪音
function fgLabel(fg){
  if (!fg || !fg.exe) return '当前前台：无';
  const e = String(fg.exe).toLowerCase();
  if (e === 'kg-ui.exe' || e === 'kg-agent.exe') return 'KeyGuard 界面（本程序）';
  return esc(fg.exe);
}
function esc(s){`,
);

fs.writeFileSync(p, s);
console.log("ok");
