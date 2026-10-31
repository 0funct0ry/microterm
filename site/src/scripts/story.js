// Animated scenes for the "Configure in TOML, script from the shell" section.
(() => {
const sleep = ms => new Promise(r => setTimeout(r, ms));
const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
const seen = new Map();
const io = new IntersectionObserver(es => es.forEach(e => seen.set(e.target, e.isIntersecting)), { rootMargin: "120px" });
const gate = async el => { while (!seen.get(el)) await sleep(250); };
const TH = {"Tokyo Night":["#1a1b26","#c0caf5","#7aa2f7"], "Nord":["#2e3440","#d8dee9","#88c0d0"], "Gruvbox Dark":["#282828","#ebdbb2","#83a598"], "One Light":["#fafafa","#242529","#5c78e2"]};
const OUT = '<span class="a">payments-api</span> main ❯ cargo test\nrunning 42 tests ... ok\nsee <span class="cs-lk on">ISSUE-12</span> for details';
const colour = (W, n) => { const c = TH[n]; W.style.setProperty("--bg", c[0]); W.style.setProperty("--fg", c[1]); W.style.setProperty("--ac", c[2]); };
const flash = W => { const t = W.querySelector(".cs-toast"); t.classList.add("on"); setTimeout(() => t.classList.remove("on"), 1400); };
const place = (L, wrap, el, label) => {
  const wr = wrap.getBoundingClientRect(), er = el.getBoundingClientRect();
  L.textContent = label; L.style.left = Math.max(6, er.left - wr.left) + "px"; L.style.top = Math.max(2, er.top - wr.top - 22) + "px"; L.classList.add("on");
};

/* scene 2: each setting maps to something visible */
function map(root) {
  const W = root.querySelector(".cs-mw"), wrap = root.querySelector(".cs-win"), L = root.querySelector(".cs-lbl"), pre = W.querySelector(".cs-out"), tabs = W.querySelector(".cs-tabs");
  const lines = [...root.querySelectorAll(".cs-ln")];
  const steps = [["theme", "colours", () => W], ["font", "text", () => pre], ["link", "links", () => pre.querySelector(".cs-lk")], ["tabs", "tabs", () => tabs]];
  (async () => { for (let i = 0; ; i = (i + 1) % steps.length) {
    await gate(root);
    const [id, label, part] = steps[i];
    lines.forEach(l => l.classList.toggle("hot", l.dataset.p === id));
    root.querySelectorAll(".cs-glow").forEach(e => e.classList.remove("cs-glow"));
    const el = part(); el.classList.add("cs-glow"); place(L, wrap, el, label);
    await sleep(2200); L.classList.remove("on");
  } })();
}

/* scene 3: edit the file and the app restyles */
function live(root) {
  const T = root.querySelector(".cs-live"), W = root.querySelector(".cs-mw"), pre = W.querySelector(".cs-out");
  const seq = [{size:13.5,th:"Tokyo Night"}, {size:16,th:"Tokyo Night",h:"size"}, {size:16,th:"Nord",h:"theme"}, {size:12,th:"Gruvbox Dark",h:"both"}, {size:14,th:"One Light",h:"both"}];
  (async () => { for (let i = 0; ; i = (i + 1) % seq.length) {
    await gate(root);
    const s = seq[i], hot = k => (s.h === k || s.h === "both") ? " hot" : "";
    T.innerHTML = `<span class="cs-ln${hot("theme")}"><span class="k">theme</span> = <span class="str">"${s.th}"</span></span><span class="cs-ln"> </span><span class="cs-ln"><span class="sec">[font]</span></span><span class="cs-ln"><span class="k">family</span> = <span class="str">"JetBrains Mono"</span></span><span class="cs-ln${hot("size")}"><span class="k">size</span>   = <span class="num">${s.size}</span></span><span class="cs-ln"><span class="k">ligatures</span> = <span class="k">true</span></span>`;
    colour(W, s.th); pre.style.setProperty("--fs", s.size + "px"); if (s.h) flash(W);
    await sleep(2600);
  } })();
}

/* scene 4: shell commands drive the app */
function shell(root) {
  const T = root.querySelector(".cs-term"), W = root.querySelector(".cs-mw"), dot = root.querySelector(".cs-dot"), tabs = W.querySelector(".cs-tabs"), pre = W.querySelector(".cs-out"), chip = W.querySelector(".cs-chip");
  const P = '<span class="p">$</span> ';
  const type = async t => { T.innerHTML += P; for (const ch of t) { T.innerHTML += ch; await sleep(26); } T.innerHTML += "\n"; };
  const fire = async () => { dot.classList.remove("go"); void dot.offsetWidth; dot.classList.add("go"); await sleep(900); };
  (async () => { for (;;) {
    await gate(root);
    T.innerHTML = ""; chip.textContent = "payments"; tabs.innerHTML = '<span class="on">api</span>'; pre.innerHTML = '<span class="a">payments-api</span> main ❯ ';
    await sleep(700); await type("microterm ctl tab new --title tests -- cargo test"); await fire();
    tabs.innerHTML = '<span>api</span><span class="on">tests</span>'; pre.innerHTML = 'running 42 tests ... ok\ntest result: <span class="a">ok</span>'; T.innerHTML += '<span class="d">14</span>\n';
    await sleep(1100); await type("microterm ctl pane get-text --pane 14 | tail -2"); await fire();
    T.innerHTML += '<span class="d">running 42 tests ... ok\ntest result: ok</span>\n';
    await sleep(1200); await type("microterm ctl workspace switch infra"); await fire();
    chip.textContent = "infra"; tabs.innerHTML = '<span class="on">shell</span>'; pre.innerHTML = '<span class="a">infra</span> main ❯ ';
    await sleep(2400);
  } })();
}

const scenes = { "cs-map": map, "cs-live": live, "cs-shell": shell };
if (!reduced) Object.entries(scenes).forEach(([id, run]) => { const el = document.getElementById(id); if (el) { io.observe(el); run(el); } });
})();
