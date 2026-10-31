// The dashboard's only script: the "Saved" toast, two-click deletes, the phone menu, marking
// the current page in the sidebar, and the timezone cookie.
(() => {
  const toast = (text) => {
    const t = document.createElement("div");
    t.className = "toast";
    t.textContent = text;
    document.body.append(t);
    requestAnimationFrame(() => t.classList.add("show"));
    setTimeout(() => { t.classList.remove("show"); setTimeout(() => t.remove(), 250); }, 1800);
  };
  window.botToast = toast;

  // Tell the server the browser's timezone so times read in local time.
  try {
    const tz = Intl.DateTimeFormat().resolvedOptions().timeZone;
    if (tz && !document.cookie.split("; ").includes("tz=" + encodeURIComponent(tz))) {
      document.cookie = "tz=" + encodeURIComponent(tz) + "; path=/; max-age=31536000; samesite=lax";
      if (document.querySelector("[data-local]")) location.reload();
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
    document.querySelectorAll(".nav.on").forEach((a) => a.classList.remove("on"));
    best.classList.add("on");
  }

  document.addEventListener("click", (e) => {
    const btn = e.target.closest("[data-confirm]");
    if (btn && !btn.classList.contains("armed")) {
      e.preventDefault();
      const label = btn.textContent;
      btn.classList.add("armed");
      btn.textContent = btn.dataset.confirm;
      setTimeout(() => { btn.classList.remove("armed"); btn.textContent = label; }, 3000);
      return;
    }
    const menu = e.target.closest(".menu-btn");
    if (menu) {
      const side = menu.closest(".side");
      side.classList.toggle("open");
      menu.setAttribute("aria-expanded", side.classList.contains("open"));
    }
    if (!e.target.closest(".switcher")) document.querySelectorAll(".switcher[open]").forEach((d) => d.removeAttribute("open"));
  });

  // Switches marked data-autosubmit save as soon as they're flipped.
  document.addEventListener("change", (e) => {
    const el = e.target.closest("[data-autosubmit]");
    if (el) el.form.requestSubmit();
  });
})();
