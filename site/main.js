// Hero demo: one dictation, then one translation, on a loop, in the app's pill colors.
const STEPS = [
  { mode: "dictate", state: "listen", c: "--violet", title: "Listening", sub: "0:04", out: "" },
  {
    mode: "dictate",
    state: "work",
    c: "--cyan",
    title: "Transcribing",
    sub: "On this device",
    out: "",
  },
  {
    mode: "dictate",
    state: "done",
    c: "--green",
    title: "Pasted",
    sub: "Natural speech",
    out: "Let's move the call to 4:30 PM and invite the design team.",
  },
  {
    mode: "translate",
    state: "listen",
    c: "--orange",
    title: "Listening",
    sub: "UK → EN",
    out: "",
  },
  {
    mode: "translate",
    state: "work",
    c: "--pink",
    title: "Translating",
    sub: "Ukrainian → English",
    out: "",
  },
  {
    mode: "translate",
    state: "done",
    c: "--green",
    title: "Pasted",
    sub: "Translated offline",
    out: "Thanks for the quick reply. I'll send the report tomorrow morning.",
  },
];
const DURATIONS = { listen: 2600, work: 1400, done: 3000 };

const pill = document.getElementById("pill");
const demo = pill?.parentElement;
const title = document.getElementById("pill-title");
const sub = document.getElementById("pill-sub");
const out = document.getElementById("demo-out");
const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

function show(i) {
  const s = STEPS[i];
  pill.dataset.state = s.state;
  pill.style.setProperty("--c", `var(${s.c})`);
  demo.dataset.mode = s.mode;
  title.textContent = s.title;
  sub.textContent = s.sub;
  out.style.opacity = s.out ? "1" : "0";
  if (s.out) out.textContent = s.out;
  if (!reduceMotion) setTimeout(() => show((i + 1) % STEPS.length), DURATIONS[s.state]);
}
if (pill) show(reduceMotion ? 2 : 0);

// Name the visitor's computer on the free download button.
const ua = navigator.userAgent;
const os =
  /Mac/.test(ua) && !/iPhone|iPad/.test(ua)
    ? "Mac"
    : /Windows/.test(ua)
      ? "Windows"
      : /Linux/.test(ua) && !/Android/.test(ua)
        ? "Linux"
        : null;
const dl = document.getElementById("desktop-download");
if (dl && os) dl.textContent = `Download for ${os}`;
