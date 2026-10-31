// The dashboard's only script: the "Saved" toast, two-click deletes, the phone menu, marking
// the current page in the sidebar, closing popovers, and the timezone cookie.
(() => {
  const toast = (text) => {
    const t = document.createElement("div");
    t.className = "toast";
    t.setAttribute("role", "status");
    t.textContent = text;
    document.body.append(t);
    requestAnimationFrame(() => t.classList.add("show"));
    setTimeout(() => { t.classList.remove("show"); setTimeout(() => t.remove(), 250); }, 1800);
  };
  window.botToast = toast;

  // Tell the server the browser's timezone so times read in local time. Reload only once the
  // cookie has stuck, so a browser that refuses cookies doesn't reload forever.
  try {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    const want = "tz=" + encodeURIComponent(tz);
    const has = () => document.cookie.split("; ").includes(want);
    if (tz && !has()) {
      document.cookie = want + "; path=/; max-age=31536000; samesite=lax";
      if (has() && document.querySelector("[data-local]")) location.reload();
    }
  } catch (_) {}

  const url = new URL(location.href);
  if (url.searchParams.has("saved")) {
    toast("Saved");
    url.searchParams.delete("saved");
    history.replaceState(null, "", url.pathname + url.search + url.hash);
  }

  // Longest matching sidebar link wins, so /g/1/pets/approvals marks "Approvals", not "Pets".
  let best = null;
  document.querySelectorAll(".nav[data-path]").forEach((a) => {
    const p = a.dataset.path;
    if ((location.pathname === p || location.pathname.startsWith(p + "/")) && (!best || p.length > best.dataset.path.length)) best = a;
  });
  if (best) {
    document.querySelectorAll(".nav.on").forEach((a) => { a.classList.remove("on"); a.removeAttribute("aria-current"); });
    best.classList.add("on");
    best.setAttribute("aria-current", "page");
    best.closest(".nav-fold")?.setAttribute("open", "");
  }

  // Sidebar groups remember being opened or folded, per browser. The current page's group
  // always starts open.
  const folds = [...document.querySelectorAll(".nav-fold")];
  const FOLDS = "nav-open";
  let opened = [];
  try { opened = JSON.parse(localStorage.getItem(FOLDS) || "[]"); } catch (_) {}
  folds.forEach((d) => { if (opened.includes(d.dataset.group)) d.setAttribute("open", ""); });
  let searching = false;
  folds.forEach((d) => d.addEventListener("toggle", () => {
    if (searching) return;
    try { localStorage.setItem(FOLDS, JSON.stringify(folds.filter((f) => f.open).map((f) => f.dataset.group))); } catch (_) {}
  }));

  // Page search: matches names and what each page does, opens groups that hold a match, and
  // Enter goes to the first one.
  const q = document.getElementById("nav-q");
  if (q) {
    const side = q.closest(".side");
    const links = [...side.querySelectorAll(".side-links .nav")];
    const none = side.querySelector(".nav-none");
    let before = null;
    const visible = () => links.filter((a) => !a.hidden);
    q.addEventListener("input", () => {
      const words = q.value.trim().toLowerCase().split(/\s+/).filter(Boolean);
      if (words.length && !searching) { searching = true; before = folds.map((d) => d.open); }
      links.forEach((a) => {
        const hay = (a.textContent + " " + (a.dataset.about || "")).toLowerCase();
        a.hidden = !words.every((w) => hay.includes(w));
      });
      folds.forEach((d) => {
        const any = [...d.querySelectorAll(".nav")].some((a) => !a.hidden);
        d.hidden = words.length > 0 && !any;
        if (words.length) d.open = true;
      });
      side.classList.toggle("searching", words.length > 0);
      none.hidden = !words.length || visible().length > 0;
      if (!words.length && searching) {
        folds.forEach((d, i) => { d.open = before[i]; });
        searching = false;
      }
    });
    q.addEventListener("keydown", (e) => {
      if (e.key === "Enter") { const first = visible()[0]; if (first) location.href = first.href; }
      if (e.key === "Escape") { q.value = ""; q.dispatchEvent(new Event("input")); q.blur(); }
      if (e.key === "ArrowDown") { e.preventDefault(); visible()[0]?.focus(); }
    });
    document.addEventListener("keydown", (e) => {
      if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target;
      if (t.closest("input, textarea, select, [contenteditable]")) return;
      e.preventDefault();
      side.classList.add("open");
      q.focus();
    });
  }

  // Popovers and the server switcher close on a click outside them or on Escape.
  const closeOpen = (except) => document.querySelectorAll(".switcher[open], .pop[open]").forEach((d) => { if (!d.contains(except)) d.removeAttribute("open"); });

  document.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-confirm]");
    if (btn && !btn.classList.contains("armed")) {
      e.preventDefault();
      // Keep the markup, not just the text, so icon buttons get their icon back.
      const html = btn.innerHTML;
      const aria = btn.getAttribute("aria-label");
      btn.classList.add("armed");
      btn.textContent = btn.dataset.confirm;
      if (aria) btn.setAttribute("aria-label", btn.dataset.confirm);
      setTimeout(() => {
        btn.classList.remove("armed");
        btn.innerHTML = html;
        if (aria) btn.setAttribute("aria-label", aria);
      }, 3000);
      return;
    }
    const menu = e.target.closest(".menu-btn");
    if (menu) {
      const side = menu.closest(".side");
      side.classList.toggle("open");
      menu.setAttribute("aria-expanded", side.classList.contains("open"));
    }
    closeOpen(e.target);
  });

  document.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return;
    const open = document.querySelector(".switcher[open], .pop[open]");
    if (!open) return;
    open.removeAttribute("open");
    open.querySelector("summary")?.focus();
  });

  // Switches marked data-autosubmit save as soon as they're flipped.
  document.addEventListener("change", (e) => {
    const el = e.target.closest("[data-autosubmit]");
    if (el) el.form.requestSubmit();
  });
})();
