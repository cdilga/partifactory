// The share-sheet island: progressive enhancement over the askama-rendered sheet.
// No framework. The server renders the current state; this wires the clicks.
const CHIPS = ["Only me", "3 people", "Family", "Everyone signed in · 15", "Public link"];
const sentence = (rung, contribute) => [
  "Only you can see this.",
  "You and 3 people can see this. A family member can also edit.",
  "You and the Family group (6) can see this.",
  "Everyone signed in (15) can see this.",
  contribute
    ? "Anyone with the link can open and use this, without signing in. Only you and people you add can change it."
    : "Anyone with the link can open this, without signing in.",
][rung];

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => [...r.querySelectorAll(s)];
const sheet = $("[data-share]");
let rung = $$("[data-rung]").findIndex((b) => b.classList.contains("on"));
let contribute = false;

function render() {
  $$("[data-rung]").forEach((b, i) => {
    b.classList.toggle("on", i === rung);
    b.setAttribute("aria-checked", String(i === rung));
  });
  $$("[data-mode]").forEach((b) => b.classList.toggle("on", (b.dataset.mode === "contribute") === contribute));
  $("[data-sentence]").textContent = sentence(rung, contribute);
  $("[data-public]").hidden = rung !== 4;
  $(".chip").textContent = CHIPS[rung];
}

$$("[data-open-share]").forEach((b) => b.addEventListener("click", () => sheet.classList.add("open")));
$("[data-close]").addEventListener("click", () => sheet.classList.remove("open"));
$("[data-menu]").addEventListener("click", () => { const m = $(".menu"); m.hidden = !m.hidden; });
$$("[data-rung]").forEach((b, i) => b.addEventListener("click", () => { rung = i; resetStepUp(); render(); }));
$$("[data-mode]").forEach((b) => b.addEventListener("click", () => { contribute = b.dataset.mode === "contribute"; render(); }));
$("[data-copy]").addEventListener("click", async (e) => {
  e.target.textContent = "Copied";
  try { await navigator.clipboard.writeText("https://photofinder-part.example.com/"); } catch {}
  setTimeout(() => (e.target.textContent = "Copy"), 1500);
});
const stepBtn = $("[data-stepup]");
function resetStepUp() { stepBtn.disabled = false; stepBtn.textContent = "Turn on public link"; stepBtn.classList.remove("ok"); }
stepBtn.addEventListener("click", () => {
  // Fake in-sheet step-up: the real one calls navigator.credentials.get() here.
  stepBtn.disabled = true;
  stepBtn.textContent = "Confirm it's you…";
  setTimeout(() => { stepBtn.textContent = "Public link is on."; stepBtn.classList.add("ok"); }, 800);
});
render();
