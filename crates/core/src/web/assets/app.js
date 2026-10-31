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
