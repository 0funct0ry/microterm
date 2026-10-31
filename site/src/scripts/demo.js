// Marketing page behaviour: shortcut badges, page theme, shortcut keycaps and install tabs.
(() => {
const isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
const KEYS = isMac
  ? {qs:"⌘⇧Space", cm:"⌘⇧X", pal:"⌘⇧P", nt:"⌘T", ws:"⌘⌥W"}
  : {qs:"Ctrl+Shift+Space", cm:"Ctrl+Shift+X", pal:"Ctrl+Shift+P", nt:"Ctrl+Shift+T", ws:"Ctrl+Alt+W"};
document.querySelectorAll("kbd[data-k]").forEach(k => k.textContent = KEYS[k.dataset.k]);

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

/* shortcut keycaps */
const KEYCAPS = isMac
  ? {pal:["⌘","⇧","P"], qs:["⌘","⇧","Space"], cm:["⌘","⇧","X"], la:["⌘","⇧","L"], ws:["⌘","⌥","W"], sr:["⌘","D"], sd:["⌘","⇧","D"], nt:["⌘","T"]}
  : {pal:["Ctrl","Shift","P"], qs:["Ctrl","Shift","Space"], cm:["Ctrl","Shift","X"], la:["Ctrl","Shift","L"], ws:["Ctrl","Alt","W"], sr:["Ctrl","Shift","D"], sd:["Ctrl","Shift","E"], nt:["Ctrl","Shift","T"]};
document.querySelectorAll("[data-keys]").forEach(el => {
  const i = el.parentElement.style.getPropertyValue("--i");
  el.innerHTML = KEYCAPS[el.dataset.keys].map(k => `<kbd style="--i:${i}">${k}</kbd>`).join("");
});

/* install tabs */
const V = document.documentElement.dataset.version || "1.0.0";
const INST = [
  `<span class="cm"># macOS 12 or later, Apple Silicon or Intel</span>\nbrew install --cask 0funct0ry/tap/microterm`,
  `<span class="cm"># Linux: needs a Vulkan driver (Mesa is fine)</span>\nbrew install 0funct0ry/tap/microterm`,
  `<span class="cm"># Detects your OS and CPU, verifies the checksum, installs to ~/.local</span>\ncurl -fsSL https://0funct0ry.github.io/microterm/install.sh | sh`,
  `<span class="cm"># Download microterm-${V}-aarch64-apple-darwin.dmg (x86_64 for Intel), open it,\n# and drag microterm.app to Applications. Then, once:</span>\nxattr -dr com.apple.quarantine /Applications/microterm.app\n<span class="cm"># First launch adds microterm and mt to ~/.local/bin; open a new shell to use them.</span>`,
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

showInst(isMac?0:2);
})();
