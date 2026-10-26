// Interactive terminal demo for the marketing page, from MARKETING-MOCKUP.html.
(() => {
const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const KEYS = isMac
  ? {qs:"⌘⇧Space", cm:"⌘⇧X", pal:"⌘⇧P", nt:"⌘T", ws:"⌘⌥W"}
  : {qs:"Ctrl+Shift+Space", cm:"Ctrl+Shift+X", pal:"Ctrl+Shift+P", nt:"Ctrl+Shift+T", ws:"Ctrl+Alt+W"};
document.querySelectorAll("kbd[data-k]").forEach(k => k.textContent = KEYS[k.dataset.k]);
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

/* page theme toggle */
const root = document.documentElement;
document.getElementById("pageTheme").onclick = () => {
  const dark = root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  root.dataset.theme = dark ? "light" : "dark";
  try { localStorage.setItem("starlight-theme", root.dataset.theme); } catch (e) {}
};

/* copy helpers */
async function copyText(t){ try{ await navigator.clipboard.writeText(t); return true }catch(e){ return false } }
document.querySelectorAll(".copy[data-copy]").forEach(b => b.onclick = async () => {
  await copyText(b.dataset.copy); const o=b.textContent; b.textContent="Copied"; setTimeout(()=>b.textContent=o,1400);
});

/* ===== terminal themes ===== */
const THEMES = [
  {n:"One Dark", bg:"#282c33", fg:"#dce0e5", a:["#282c33","#d07277","#a1c181","#dec184","#74ade8","#b477cf","#6eb4bf","#dce0e5","#6b717d"]},
  {n:"One Light", bg:"#fafafa", fg:"#242529", a:["#242529","#d36151","#669f59","#a48819","#5c78e2","#984ea5","#3a82b7","#58585a","#8a8c94"]},
  {n:"Tokyo Night", bg:"#1a1b26", fg:"#c0caf5", a:["#15161e","#f7768e","#9ece6a","#e0af68","#7aa2f7","#bb9af7","#7dcfff","#a9b1d6","#565f89"]},
  {n:"Catppuccin Mocha", bg:"#1e1e2e", fg:"#cdd6f4", a:["#45475a","#f38ba8","#a6e3a1","#f9e2af","#89b4fa","#f5c2e7","#94e2d5","#bac2de","#6c7086"]},
  {n:"Gruvbox Dark", bg:"#282828", fg:"#ebdbb2", a:["#282828","#fb4934","#b8bb26","#fabd2f","#83a598","#d3869b","#8ec07c","#ebdbb2","#928374"]},
  {n:"Kanagawa Wave", bg:"#1f1f28", fg:"#dcd7ba", a:["#16161d","#e46876","#98bb6c","#e6c384","#7e9cd8","#957fb8","#7aa89f","#c8c093","#727169"]},
  {n:"Nord", bg:"#2e3440", fg:"#d8dee9", a:["#3b4252","#bf616a","#a3be8c","#ebcb8b","#81a1c1","#b48ead","#88c0d0","#e5e9f0","#616e88"]},
  {n:"Rosé Pine Dawn", bg:"#faf4ed", fg:"#575279", a:["#575279","#b4637a","#286983","#ea9d34","#56949f","#907aa9","#d7827e","#797593","#9893a5"]},
  {n:"Flexoki Light", bg:"#fffcf0", fg:"#100f0f", a:["#100f0f","#af3029","#66800b","#ad8301","#205ea6","#a02f6f","#24837b","#575653","#878580"]},
];
const hex = h => [1,3,5].map(i=>parseInt(h.slice(i,i+2),16));
const mix = (a,b,t) => { const A=hex(a),B=hex(b); return "#"+A.map((v,i)=>Math.round(v+(B[i]-v)*t).toString(16).padStart(2,"0")).join("") };
const term = document.getElementById("term");
function applyTheme(i){
  const t = THEMES[i], s = term.style;
  s.setProperty("--t-bg", t.bg); s.setProperty("--t-fg", t.fg);
  s.setProperty("--t-surface", mix(t.bg,t.fg,.06)); s.setProperty("--t-border", mix(t.bg,t.fg,.18));
  s.setProperty("--t-muted", mix(t.fg,t.bg,.35)); s.setProperty("--t-accent", t.a[4]);
  t.a.forEach((c,k)=> s.setProperty("--c"+k, c));
  document.querySelectorAll(".sw").forEach((b,k)=>b.setAttribute("aria-pressed", k===i));
  document.getElementById("themeName").textContent = t.n;
}
const swWrap = document.getElementById("swatches");
THEMES.forEach((t,i) => {
  const b = document.createElement("button");
  b.className="sw"; b.style.background=t.bg; b.title=t.n; b.setAttribute("aria-label","Theme "+t.n);
  b.innerHTML = `<span style="background:linear-gradient(90deg,${t.a[1]} 0 20%,${t.a[2]} 20% 40%,${t.a[3]} 40% 60%,${t.a[4]} 60% 80%,${t.a[5]} 80%)"></span>`;
  b.onclick = () => applyTheme(i);
  swWrap.appendChild(b);
});
const tn = document.createElement("span"); tn.className="theme-name"; tn.id="themeName"; swWrap.appendChild(tn);

/* ===== content ===== */
const P = (path, branch) => `<span class="c4 b">${path}</span>${branch?` <span class="c5">${branch}</span>`:""} <span class="c2">❯</span> `;
const q = t => `<span class="qsm">${t}</span>`;
const CONTENT = {
  zsh: () => [
    P("~/src/payments-api","main") + "git log --oneline -6",
    `<span class="c3">${q("3f2a9c1")}</span> (<span class="c6 b">HEAD -&gt; </span><span class="c2 b">main</span>) fix: retry settlement on 503`,
    `<span class="c3">${q("b81e04d")}</span> feat(bbps): add biller fetch timeout`,
    `<span class="c3">${q("9c0d7e2")}</span> refactor: split ledger writer`,
    `<span class="c3">${q("e4a1f33")}</span> test: cover duplicate refund path`,
    `<span class="c3">${q("71bd0a8")}</span> chore: bump dependencies`,
    `<span class="c3">${q("a0c9f5e")}</span> docs: settlement runbook (<span class="lnk">PAY-212</span>)`,
    P("~/src/payments-api","main") + "curl -s localhost:8080/healthz | jq",
    `{`,
    `  <span class="c4">"status"</span>: <span class="c2">"ok"</span>,`,
    `  <span class="c4">"node"</span>: <span class="c2">"${q("10.0.12.4")}"</span>,`,
    `  <span class="c4">"build"</span>: <span class="c2">"${q("5f0c2d1e-8b3a-4c1f-9e2d-7a6b5c4d3e2f")}"</span>,`,
    `  <span class="c4">"docs"</span>: <span class="c2">"<span class="lnk">${q("https://internal.example.com/runbooks/settlement")}</span>"</span>`,
    `}`,
    P("~/src/payments-api","main") + `<span class="cur"> </span>`,
  ],
  watch: () => [
    `<span class="c8">[Running 'cargo check']</span>`,
    `<span class="c2 b">    Checking</span> payments-api v0.9.2 (${q("/home/dev/src/payments-api")})`,
    `<span class="c3 b">warning</span><span class="b">: unused variable: </span><span class="uc">\`retries\`</span>`,
    `  <span class="c4 b">--&gt;</span> ${q("src/ledger/writer.rs:42:9")}`,
    `   <span class="c4 b">|</span>`,
    `<span class="c4 b">42 |</span>     let <span class="uc">retries</span> = policy.max_attempts();`,
    `   <span class="c4 b">|</span>         <span class="c3 b">^^^^^^^</span> <span class="c3 b">help: prefix it with an underscore: \`_retries\`</span>`,
    ``,
    `<span class="c1 b">error[E0308]</span><span class="b">: mismatched types</span>`,
    `  <span class="c4 b">--&gt;</span> ${q("src/bbps/fetch.rs:118:24")}`,
    `<span class="c4 b">118 |</span>     Ok(<span class="ucr">timeout_ms</span>)`,
    `    <span class="c4 b">|</span>        <span class="c1 b">^^^^^^^^^^ expected \`Duration\`, found \`u64\`</span>`,
    ``,
    `Rich text: <span class="b">bold</span> <span class="it">italic</span> <span class="ud">double</span> <span class="uc">curly</span> <span class="s">struck</span> <span class="dim">dim</span> <span class="c5">ポ 🦀</span>`,
    `<span class="c8">[Finished running. Exit status: 101]</span>`,
  ],
  prod: () => [
    `<span class="c2 b">deploy@prod-01</span> <span class="c4 b">~</span> <span class="c2">❯</span> kubectl get pods -o wide -n payments`,
    `<span class="b">NAME                         READY   STATUS    RESTARTS   IP             NODE</span>`,
    `settlement-7d9c6b5f4-2xkqp   1/1     <span class="c2">Running</span>   0          ${q("10.42.3.17")}     ${q("ip-10-0-1-21")}`,
    `settlement-7d9c6b5f4-9fz8m   1/1     <span class="c2">Running</span>   0          ${q("10.42.1.88")}     ${q("ip-10-0-2-14")}`,
    `biller-fetch-5c8d7f9b-q4l2n  0/1     <span class="c1">CrashLoopBackOff</span>  7  ${q("10.42.2.40")}     ${q("ip-10-0-3-9")}`,
    `ledger-writer-6f4b8c-hm7td   1/1     <span class="c2">Running</span>   1          ${q("10.42.3.52")}     ${q("ip-10-0-1-21")}`,
    `<span class="c2 b">deploy@prod-01</span> <span class="c4 b">~</span> <span class="c2">❯</span> top -bn1 | head -5`,
    `top - 14:02:11 up 41 days,  3 users,  load average: <span class="c3">0.42</span>, 0.38, 0.35`,
    `Tasks: <span class="b">213</span> total,   1 running, 212 sleeping`,
    `%Cpu(s):  4.1 us,  1.2 sy,  0.0 ni, 94.4 id`,
    `MiB Mem :  15893.4 total,   3120.2 free,   7012.8 used`,
    `<span class="c2 b">deploy@prod-01</span> <span class="c4 b">~</span> <span class="c2">❯</span> <span class="cur"> </span>`,
  ],
  tf: () => [
    `<span class="c4 b">~/src/infra</span> <span class="c5">main</span> <span class="c2">❯</span> terraform plan -target=module.vpc`,
    `<span class="b">Terraform will perform the following actions:</span>`,
    ``,
    `  <span class="c3"># module.vpc.aws_subnet.private[2]</span> will be <span class="c3 b">updated in-place</span>`,
    `  <span class="c3">~</span> resource "aws_subnet" "private" {`,
    `      id         = "${q("subnet-0a7f3c9e1b2d4f6a8")}"`,
    `    <span class="c3">~</span> cidr_block = "<span class="s">${"10.0.32.0/20"}</span>" <span class="c3">-&gt;</span> "${q("10.0.48.0/20")}"`,
    `    }`,
    ``,
    `<span class="b">Plan:</span> 0 to add, <span class="c3">1 to change</span>, 0 to destroy.`,
    `<span class="c4 b">~/src/infra</span> <span class="c5">main</span> <span class="c2">❯</span> <span class="cur"> </span>`,
  ],
  fresh: () => [`<span class="c4 b">~</span> <span class="c2">❯</span> <span class="cur"> </span>`],
};

let workspaces = {
  payments: {tabs:[{t:"zsh",c:"zsh",dir:"~/src/payments-api",dom:"local"},{t:"cargo watch",c:"watch",dir:"~/src/payments-api",dom:"local",busy:true},{t:"ssh: prod",c:"prod",dir:"~",dom:"prod"}], active:0},
  infra: {tabs:[{t:"terraform",c:"tf",dir:"~/src/infra",dom:"local"}], active:0},
};
let ws = "payments";
const grid = document.getElementById("grid"), layer = document.getElementById("layer");
const tabsEl = document.getElementById("tabs");

function renderTabs(){
  const W = workspaces[ws];
  tabsEl.innerHTML = "";
  W.tabs.forEach((tb,i) => {
    const b = document.createElement("button");
    b.className="tab"; b.setAttribute("role","tab"); b.setAttribute("aria-selected", i===W.active);
    b.innerHTML = `${tb.busy?'<span class="busy" title="Process running"></span>':""}<span>${tb.t}</span><span class="x" aria-label="Close tab">×</span>`;
    b.onclick = e => { if (e.target.classList.contains("x")) return closeTab(i); activate(i); };
    tabsEl.appendChild(b);
  });
  const add = document.createElement("button"); add.className="tab-add"; add.textContent="+"; add.title="New tab"; add.setAttribute("aria-label","New tab");
  add.onclick = newTab; tabsEl.appendChild(add);
  document.getElementById("wsName").textContent = ws;
}
function setDomain(dom, state){
  document.getElementById("domName").textContent = dom;
  document.getElementById("domDot").className = "dot " + (state||"ok");
}
function activate(i){
  exitModes();
  const W = workspaces[ws]; W.active = i; renderTabs();
  const tb = W.tabs[i];
  grid.innerHTML = CONTENT[tb.c]().map(l => `<div class="ln">${l}</div>`).join("");
  document.getElementById("tbTitle").textContent = `${tb.dir} — ${tb.t.startsWith("ssh")?"zsh":tb.t}`;
  setDomain(tb.dom, "ok");
}
function newTab(){ const W=workspaces[ws]; W.tabs.push({t:"zsh",c:"fresh",dir:"~",dom:"local"}); activate(W.tabs.length-1); }
function closeTab(i){
  const W=workspaces[ws]; if (W.tabs.length===1){ toast("Last tab in workspace kept open in the demo"); return }
  W.tabs.splice(i,1); activate(Math.min(W.active, W.tabs.length-1));
}
function switchWs(name){ ws=name; activate(workspaces[ws].active); toast(`Switched to workspace ${name}`); }
document.getElementById("wsChip").onclick = () => { openPalette("@"); };

/* toasts */
function toast(msg){
  const t = document.createElement("div"); t.className="toast"; t.textContent=msg;
  const box=document.getElementById("toasts"); box.appendChild(t);
  while(box.children.length>3) box.firstChild.remove();
  setTimeout(()=>t.remove(), 3200);
}

/* typing intro */
function intro(){
  activate(0);
  if (reduced) return;
  const lines = [...grid.children];
  const first = lines[0], full = first.innerHTML, cmd = "git log --oneline -6";
  const prefix = full.slice(0, full.length - cmd.length);
  lines.slice(1).forEach(l => l.style.visibility="hidden");
  let k=0; first.innerHTML = prefix + '<span class="cur"> </span>';
  const iv = setInterval(() => {
    k++; first.innerHTML = prefix + cmd.slice(0,k) + (k<cmd.length?'<span class="cur"> </span>':"");
    if (k>=cmd.length){ clearInterval(iv);
      lines.slice(1).forEach((l,j)=> setTimeout(()=> l.style.visibility="visible", 220 + j*35));
    }
  }, 55);
}

/* ===== modes ===== */
let mode = null; // "qs" | "cm" | "pal"
const pill = document.getElementById("pill");
function exitModes(){
  mode=null; layer.innerHTML=""; pill.classList.remove("on");
  document.getElementById("palette").classList.remove("on");
}
let cw = 7.8;
function measure(){
  const s=document.createElement("span"); s.textContent="MMMMMMMMMM"; s.style.visibility="hidden";
  grid.appendChild(s); cw = s.getBoundingClientRect().width/10; s.remove();
}
const PAD_X = 12, PAD_Y = 8, LH = 20;

/* quick select */
const ALPHA = "asdfqwerzxcvjklmiuopghtybn";
let qsMap = {};
function startQS(){
  exitModes(); mode="qs"; pill.textContent="Quick select"; pill.classList.add("on"); qsMap={};
  const ms = [...grid.querySelectorAll(".qsm")];
  const gr = grid.getBoundingClientRect();
  const withPos = ms.map(m => { const r=m.getBoundingClientRect(); return {m, x:r.left-gr.left, y:r.top-gr.top, w:r.width} });
  withPos.sort((a,b)=> b.y-a.y || a.x-b.x);
  withPos.forEach((p,i) => {
    const lab = ALPHA[i]; if (!lab) return;
    qsMap[lab] = p.m.textContent;
    const hit=document.createElement("div"); hit.className="qs-hit"; Object.assign(hit.style,{left:p.x+"px",top:p.y+"px",width:p.w+"px"}); layer.appendChild(hit);
    const c=document.createElement("div"); c.className="qs-chip"; c.textContent=lab; Object.assign(c.style,{left:p.x+"px",top:p.y+"px"}); layer.appendChild(c);
  });
  if (!withPos.length){ toast("Nothing to select on this screen"); exitModes(); }
}
async function qsKey(k){
  const lab = k.toLowerCase(); const val = qsMap[lab];
  if (!val) return;
  await copyText(val);
  if (k !== lab){ // uppercase: copy + paste
    const cur = grid.querySelector(".cur");
    if (cur) cur.insertAdjacentText("beforebegin", val);
    toast(`Copied and pasted ${val}`);
  } else toast(`Copied ${val}`);
  exitModes();
}

/* copy mode */
let cm = {r:0,c:0,sel:null,kind:"v",lines:[],pending:""};
function startCM(){
  exitModes(); measure(); mode="cm"; pill.textContent="Copy mode"; pill.classList.add("on");
  cm.lines = [...grid.children].map(l=>l.textContent);
  cm.r = cm.lines.length-1; cm.c = Math.max(0, cm.lines[cm.r].trimEnd().length); cm.sel=null; cm.pending="";
  drawCM();
}
function clampC(){ cm.c = Math.max(0, Math.min(cm.c, Math.max(0, cm.lines[cm.r].length-1))) }
function isW(ch){ return /[A-Za-z0-9_]/.test(ch) }
function drawCM(){
  layer.innerHTML="";
  if (cm.sel){
    const a = cm.sel, b = {r:cm.r,c:cm.c};
    const [s,e] = (a.r<b.r || (a.r===b.r && a.c<=b.c)) ? [a,b] : [b,a];
    for (let r=s.r; r<=e.r; r++){
      let c0, c1;
      if (cm.kind==="V"){ c0=0; c1=Math.max(1,cm.lines[r].length) }
      else if (cm.kind==="b"){ c0=Math.min(a.c,b.c); c1=Math.max(a.c,b.c)+1 }
      else { c0 = r===s.r? s.c:0; c1 = r===e.r? e.c+1 : Math.max(1,cm.lines[r].length) }
      const d=document.createElement("div"); d.className="cm-sel";
      Object.assign(d.style,{left:(PAD_X+c0*cw)+"px",top:(PAD_Y+r*LH)+"px",width:((c1-c0)*cw)+"px"}); layer.appendChild(d);
    }
  }
  const cur=document.createElement("div"); cur.className="cm-cursor";
  Object.assign(cur.style,{left:(PAD_X+cm.c*cw)+"px",top:(PAD_Y+cm.r*LH)+"px",width:cw+"px"}); layer.appendChild(cur);
}
function selText(){
  const a=cm.sel,b={r:cm.r,c:cm.c};
  const [s,e]=(a.r<b.r||(a.r===b.r&&a.c<=b.c))?[a,b]:[b,a]; const out=[];
  for (let r=s.r;r<=e.r;r++){
    const L=cm.lines[r];
    if (cm.kind==="V") out.push(L.trimEnd());
    else if (cm.kind==="b") out.push(L.slice(Math.min(a.c,b.c),Math.max(a.c,b.c)+1).trimEnd());
    else out.push(L.slice(r===s.r?s.c:0, r===e.r?e.c+1:undefined).trimEnd());
  }
  return out.join("\n");
}
async function cmKey(e){
  const k=e.key, L=()=>cm.lines[cm.r];
  if (e.ctrlKey && k==="v"){ cm.kind="b"; cm.sel = cm.sel||{r:cm.r,c:cm.c}; return drawCM() }
  if (cm.pending==="g"){ cm.pending=""; if (k==="g"){ cm.r=0; cm.c=0; return drawCM() } }
  switch(k){
    case "h": case "ArrowLeft": cm.c--; break;
    case "l": case "ArrowRight": cm.c++; break;
    case "j": case "ArrowDown": cm.r=Math.min(cm.lines.length-1,cm.r+1); break;
    case "k": case "ArrowUp": cm.r=Math.max(0,cm.r-1); break;
    case "0": cm.c=0; break;
    case "^": cm.c=Math.max(0,L().search(/\S/)); break;
    case "$": cm.c=L().trimEnd().length-1; break;
    case "w": { let i=cm.c, s=L(); while(i<s.length&&isW(s[i]))i++; while(i<s.length&&!isW(s[i]))i++; if(i>=s.length&&cm.r<cm.lines.length-1){cm.r++;i=Math.max(0,cm.lines[cm.r].search(/\w/))} cm.c=i; break }
    case "b": { let i=cm.c-1, s=L(); while(i>0&&!isW(s[i]))i--; while(i>0&&isW(s[i-1]))i--; cm.c=Math.max(0,i); break }
    case "e": { let i=cm.c+1, s=L(); while(i<s.length&&!isW(s[i]))i++; while(i<s.length-1&&isW(s[i+1]))i++; cm.c=i; break }
    case "g": cm.pending="g"; return;
    case "G": cm.r=cm.lines.length-1; break;
    case "H": cm.r=0; break;
    case "L": cm.r=cm.lines.length-1; break;
    case "M": cm.r=Math.floor((cm.lines.length-1)/2); break;
    case "v": cm.kind="v"; cm.sel = cm.sel? null : {r:cm.r,c:cm.c}; break;
    case "V": cm.kind="V"; cm.sel = cm.sel? null : {r:cm.r,c:cm.c}; break;
    case "o": if (cm.sel){ const t={r:cm.r,c:cm.c}; cm.r=cm.sel.r; cm.c=cm.sel.c; cm.sel=t } break;
    case "y": case "Y": {
      if (!cm.sel){ toast("Start a selection with v, V or Ctrl-v"); return }
      const t = selText(); await copyText(t);
      toast(`Copied ${t.length} characters`);
      if (k==="y") return exitModes(); cm.sel=null; break;
    }
    case "Escape": if (cm.sel){ cm.sel=null; break } return exitModes();
    case "q": return exitModes();
    default: return;
  }
  clampC(); drawCM();
}

/* palette */
const pal = document.getElementById("palette"), palIn=document.getElementById("palIn"), palList=document.getElementById("palList");
let palItems=[], palSel=0;
function allItems(){
  const items = [
    {g:">",l:"New tab",k:KEYS.nt,run:newTab},
    {g:">",l:"Quick select",k:KEYS.qs,run:startQS},
    {g:">",l:"Copy mode",k:KEYS.cm,run:startCM},
    {g:">",l:"Reload config",k:isMac?"⌘⇧R":"Ctrl+Shift+R",run:()=>toast("Config reloaded")},
    {g:">",l:"Open config",k:isMac?"⌘,":"Ctrl+,",run:()=>toast("Opening microterm.toml in $EDITOR")},
    {g:">",l:"Increase font size",k:isMac?"⌘=":"Ctrl+=",run:()=>toast("Font size 14")},
    {g:">",l:"Connect to prod",k:"",run:()=>{ if(ws!=="payments") switchWs("payments"); activate(2) }},
  ];
  Object.keys(workspaces).forEach(n => items.push({g:"@",l:"Switch to workspace: "+n,k:"",run:()=>switchWs(n)}));
  workspaces[ws].tabs.forEach((t,i)=> items.push({g:"#",l:"Go to tab: "+t.t,k:"",run:()=>activate(i)}));
  THEMES.forEach((t,i)=> items.push({g:"~",l:"Theme: "+t.n,k:"",run:()=>applyTheme(i)}));
  return items;
}
function fuzzy(q, s){
  if (!q) return {score:0, idx:[]};
  const at = s.toLowerCase().indexOf(q.toLowerCase());
  if (at >= 0) return {score: 100 + (at===0||s[at-1]===" "?20:0) - at, idx:[...Array(q.length)].map((_,i)=>at+i)};
  let qi=0, idx=[], score=0, last=-2; const S=s.toLowerCase(), Q=q.toLowerCase();
  for (let i=0;i<S.length && qi<Q.length;i++){ if (S[i]===Q[qi]){ idx.push(i); score += (i===last+1?3:1) + (i===0||S[i-1]===" "?2:0); last=i; qi++ } }
  return qi===Q.length ? {score, idx} : null;
}
function renderPal(){
  let q = palIn.value, grp = null;
  if (/^[>@#~]/.test(q)){ grp=q[0]; q=q.slice(1).trim() }
  palItems = allItems().filter(it => !grp || it.g===grp)
    .map(it => { const [pre,name] = it.l.includes(": ") ? it.l.split(": ") : ["", it.l]; return {...it, pre, name, f: fuzzy(q,name)} }).filter(it=>it.f)
    .sort((a,b)=> b.f.score-a.f.score).slice(0,40);
  palSel = Math.min(palSel, Math.max(0,palItems.length-1));
  palList.innerHTML = palItems.length ? palItems.map((it,i)=>{
    const lab=[...it.name].map((ch,j)=> it.f.idx.includes(j)?`<mark>${ch}</mark>`:ch).join("");
    return `<li role="option" data-i="${i}" aria-selected="${i===palSel}"><span>${it.pre?`<span class="grp">${it.pre}</span>`:""}${lab}</span>${it.k?`<kbd>${it.k}</kbd>`:""}</li>`;
  }).join("") : `<div class="empty">No matching actions. Try &gt; for actions, @ workspaces, # tabs, ~ themes.</div>`;
  palList.querySelectorAll("li").forEach(li => li.onclick = () => runPal(+li.dataset.i));
  const s = palList.querySelector('[aria-selected="true"]'); if (s) s.scrollIntoView({block:"nearest"});
}
function openPalette(prefix=""){
  exitModes(); mode="pal"; pal.classList.add("on"); palIn.value=prefix; palSel=0; renderPal(); palIn.focus();
}
function runPal(i){ const it=palItems[i]; exitModes(); term.focus(); if (it) it.run(); }
palIn.addEventListener("input", ()=>{ palSel=0; renderPal() });
palIn.addEventListener("keydown", e => {
  if (e.key==="ArrowDown"){ palSel=Math.min(palItems.length-1,palSel+1); renderPal(); e.preventDefault() }
  else if (e.key==="ArrowUp"){ palSel=Math.max(0,palSel-1); renderPal(); e.preventDefault() }
  else if (e.key==="Enter"){ runPal(palSel); e.preventDefault() }
  else if (e.key==="Escape"){ exitModes(); term.focus(); e.preventDefault() }
  e.stopPropagation();
});

/* reconnect simulation */
let reconTimer=null;
function dropConnection(){
  if (ws!=="payments") { ws="payments"; }
  activate(2);
  term.scrollIntoView({behavior: reduced?"auto":"smooth", block:"center"});
  const ov=document.getElementById("recon"), msg=document.getElementById("reconMsg");
  ov.classList.add("on"); setDomain("prod","warn");
  let attempt=1, wait=2;
  clearInterval(reconTimer);
  const tick = () => {
    msg.textContent = `Reconnecting in ${wait} s (attempt ${attempt}). The remote session keeps running.`;
    wait--;
    if (wait<0){ attempt++; wait = attempt===2?3:0; if (attempt>3) return restore() }
  };
  tick(); reconTimer=setInterval(tick, 900);
}
function restore(){
  clearInterval(reconTimer); document.getElementById("recon").classList.remove("on");
  setDomain("prod","ok"); toast("Reconnected to prod. Screen and scrollback restored.");
}
document.getElementById("reconNow").onclick = restore;
document.getElementById("reconClose").onclick = () => { clearInterval(reconTimer); document.getElementById("recon").classList.remove("on"); toast("Tab closed. The remote shell is still running."); const W=workspaces.payments; W.tabs.splice(2,1); activate(0) };

/* key handling (only while the demo terminal has focus) */
term.addEventListener("keydown", async e => {
  if (mode==="pal") return;
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod && e.shiftKey && (e.code==="Space")){ e.preventDefault(); return mode==="qs"?exitModes():startQS() }
  if (mod && e.shiftKey && e.code==="KeyX"){ e.preventDefault(); return mode==="cm"?exitModes():startCM() }
  if (mod && e.shiftKey && e.code==="KeyP"){ e.preventDefault(); return openPalette() }
  if (mode==="qs"){ e.preventDefault(); if (e.key==="Escape") return exitModes(); if (/^[a-zA-Z]$/.test(e.key)) return qsKey(e.key); return }
  if (mode==="cm"){ e.preventDefault(); return cmKey(e) }
});
document.querySelectorAll("[data-act]").forEach(b => b.onclick = () => {
  const a=b.dataset.act;
  if (a==="drop") return dropConnection();
  term.focus();
  if (a==="qs") startQS(); else if (a==="cm") startCM(); else if (a==="pal") openPalette();
});

/* install tabs */
const V = document.documentElement.dataset.version || "1.0.0";
const INST = [
  `<span class="cm"># macOS 12 or later, Apple Silicon or Intel</span>\nbrew install --cask 0funct0ry/tap/microterm`,
  `<span class="cm"># Linux: needs a Vulkan driver (Mesa is fine)</span>\nbrew install 0funct0ry/tap/microterm`,
  `<span class="cm"># Detects your OS and CPU, verifies the checksum, installs to ~/.local</span>\ncurl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh`,
  `<span class="cm"># Download the archive for your platform, then:</span>\ntar -xzf microterm-${V}-x86_64-unknown-linux-gnu.tar.gz\n./microterm-${V}/bin/microterm --version\n<span class="cm"># All files and SHA256SUMS: github.com/0funct0ry/microterm/releases</span>`,
];
const instCode=document.getElementById("instCode");
function showInst(i){
  document.querySelectorAll("#instTabs button").forEach((b,k)=>b.setAttribute("aria-selected",k===i));
  instCode.innerHTML = INST[i];
}
document.querySelectorAll("#instTabs button").forEach(b => b.onclick = () => showInst(+b.dataset.i));
document.getElementById("instCopy").onclick = async e => {
  const txt = instCode.textContent.split("\n").filter(l=>!l.startsWith("#")).join("\n");
  await copyText(txt); e.target.textContent="Copied"; setTimeout(()=>e.target.textContent="Copy",1400);
};

applyTheme(0); showInst(isMac?0:2); intro();
})();
