// The dashboard's one script. Everything works without it; it adds the search palette, the
// server's colour, chart cursors and tooltips, numbers that count up, the phone drawer, the
// banner upload, button states, toasts, two-click deletes and the timezone cookie.
(() => {
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const root = document.documentElement;
  const calm = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const guild = document.body.dataset.guild;
  const store = {
    get(k, d) { try { const v = localStorage.getItem(k); return v === null ? d : JSON.parse(v); } catch (_) { return d; } },
    set(k, v) { try { localStorage.setItem(k, JSON.stringify(v)); } catch (_) {} },
  };
  const ICON_OK = '<svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><path d="m3 8.5 3 3 7-7"/></svg>';
  const ICON_BAD = '<svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round"><path d="m4 4 8 8M12 4l-8 8"/></svg>';

  // ---------------------------------------------------------------- toast
  const toast = (text, bad = false) => {
    const t = document.createElement("div");
    t.className = "toast" + (bad ? " bad" : "");
    t.setAttribute("role", "status");
    t.innerHTML = `<span class="toast-ic">${bad ? ICON_BAD : ICON_OK}</span><span></span>`;
    t.lastChild.textContent = text;
    document.body.append(t);
    requestAnimationFrame(() => requestAnimationFrame(() => t.classList.add("show")));
    setTimeout(() => { t.classList.remove("show"); setTimeout(() => t.remove(), 400); }, 2200);
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
      if (has() && $("[data-local]")) location.reload();
    }
  } catch (_) {}

  const url = new URL(location.href);
  if (url.searchParams.has("saved")) {
    toast("Saved");
    url.searchParams.delete("saved");
    history.replaceState(null, "", url.pathname + url.search + url.hash);
  }

  // ---------------------------------------------------------------- sidebar
  // Longest matching link wins, so /g/1/tickets/starboard marks Starboard, not Tickets. The
  // server marks a guess first so the pill is in place on the first paint.
  let best = null;
  $$(".side-links .nav[data-path]").forEach((a) => {
    const p = a.dataset.path;
    if ((location.pathname === p || location.pathname.startsWith(p + "/")) && (!best || p.length > best.dataset.path.length)) best = a;
  });
  if (best && !best.classList.contains("on")) {
    const pill = $(".side-links .nav-pill") || Object.assign(document.createElement("span"), { className: "nav-pill" });
    $$(".side-links .nav.on").forEach((a) => { a.classList.remove("on"); a.removeAttribute("aria-current"); });
    best.classList.add("on");
    best.setAttribute("aria-current", "page");
    best.prepend(pill);
  }
  const current = $(".side-links .nav.on");
  current?.closest(".nav-fold")?.setAttribute("open", "");
  // Keep the current page in view in a long list.
  if (current) {
    const list = $(".side-links");
    const r = current.getBoundingClientRect(), lr = list.getBoundingClientRect();
    if (r.bottom > lr.bottom - 20) list.scrollTop += r.bottom - lr.bottom + 60;
  }

  // Groups stay as the person folded them, per browser. The current page's group stays open.
  const CLOSED = "nav-closed";
  let closed = store.get(CLOSED, []);
  $$(".nav-fold").forEach((d) => {
    const g = d.dataset.group;
    if (closed.includes(g) && !d.contains(current)) d.removeAttribute("open");
    d.addEventListener("toggle", () => {
      closed = closed.filter((x) => x !== g);
      if (!d.open) closed.push(g);
      store.set(CLOSED, closed);
    });
  });

  // ---------------------------------------------------------------- phone drawer
  const drawer = (open) => {
    document.body.classList.toggle("drawer", open);
    $$(".menu-btn").forEach((b) => b.setAttribute("aria-expanded", String(open)));
  };
  $(".scrim")?.addEventListener("click", () => drawer(false));

  // ---------------------------------------------------------------- palette
  const pal = $("#palette");
  const palQ = $("#pal-q");
  const palList = $("#pal-list");
  let items = [];
  let shown = [];
  let active = 0;
  let lastFocus = null;
  const RECENT = "pal-recent";

  const collect = () => {
    const out = [];
    $$(".side-links .nav").forEach((a) => {
      out.push({
        kind: "Pages",
        label: $(".grow", a)?.textContent.trim() || a.textContent.trim(),
        about: a.dataset.about || "",
        hint: a.dataset.group || "Home",
        hue: a.closest("[data-hue]")?.dataset.hue || a.dataset.hue || "core",
        icon: $(".nav-ic", a)?.innerHTML || "",
        href: a.href,
      });
    });
    $$(".guilds-list .guild").forEach((a) => {
      const img = $("img", a);
      out.push({ kind: "Servers", label: a.dataset.tip, about: "server switch", hint: a.classList.contains("on") ? "You're here" : "Switch server", img: img?.src, letter: img ? "" : a.textContent.trim(), href: a.href });
    });
    const bell = $(".guilds .bell");
    if (bell) out.push({ kind: "Go to", label: "Notifications", about: "bell alerts unread notices", hint: bell.querySelector(".count") ? bell.querySelector(".count").textContent + " new" : "Nothing new", hue: "core", icon: bell.querySelector("svg").outerHTML, href: bell.href });
    $$(".account-menu a.menu-item").forEach((a) => {
      out.push({ kind: "Go to", label: a.textContent.trim(), about: "admin settings", hint: "Admin", hue: "core", icon: $("svg", a)?.outerHTML || "", href: a.href });
    });
    const out_ = $("[data-signout]");
    if (out_) out.push({ kind: "Go to", label: "Sign out", about: "log out leave", hint: "", hue: "safety", icon: $("svg", out_).outerHTML, run: () => out_.form.submit() });
    return out;
  };

  // Letters of the query in order inside the label; earlier, contiguous and word-start
  // matches score higher. Falls back to words of the page's description.
  const score = (item, q) => {
    if (!q) return { s: 1, marks: [] };
    const label = item.label.toLowerCase();
    let s = 0, li = 0, run = 0;
    const marks = [];
    for (const ch of q) {
      const at = label.indexOf(ch, li);
      if (at < 0) { s = -1; break; }
      run = at === li ? run + 1 : 0;
      s += 1 + run * 2 + (at === 0 || /[\s.]/.test(label[at - 1]) ? 3 : 0) - Math.min(at - li, 4) * 0.2;
      marks.push(at);
      li = at + 1;
    }
    if (s > 0) return { s: s + (label.startsWith(q) ? 6 : 0), marks };
    const words = q.split(/\s+/);
    const hay = (item.label + " " + item.about + " " + item.hint).toLowerCase();
    if (words.every((w) => hay.includes(w))) return { s: 0.5, marks: [] };
    return null;
  };
  const esc = (t) => t.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  const highlight = (label, marks) => {
    if (!marks.length) return esc(label);
    let out = "";
    [...label].forEach((c, i) => { out += marks.includes(i) ? `<mark>${esc(c)}</mark>` : esc(c); });
    return out;
  };

  const render = () => {
    const q = palQ.value.trim().toLowerCase();
    let list;
    if (!q) {
      const recent = store.get(RECENT, []).map((h) => items.find((i) => i.href === h)).filter(Boolean).slice(0, 4);
      list = [...recent.map((i) => ({ ...i, kind: "Recent", marks: [] })), ...items.filter((i) => !recent.includes(i)).map((i) => ({ ...i, marks: [] }))];
    } else {
      list = items.map((i) => ({ i, r: score(i, q) })).filter((x) => x.r).sort((a, b) => b.r.s - a.r.s).map((x) => ({ ...x.i, marks: x.r.marks, kind: q ? "Best matches" : x.i.kind }));
    }
    shown = list;
    active = Math.min(active, Math.max(0, list.length - 1));
    if (!list.length) { palList.innerHTML = `<p class="pal-none">Nothing matches “${esc(palQ.value.trim())}”.</p>`; return; }
    let html = "", last = "";
    list.forEach((it, n) => {
      if (it.kind !== last) { html += `<p class="pal-group">${esc(it.kind)}</p>`; last = it.kind; }
      const face = it.img ? `<img class="gicon" src="${esc(it.img)}" alt="">` : it.letter !== undefined && !it.icon ? `<span class="nav-ic" data-hue="core">${esc(it.letter || "")}</span>` : `<span class="nav-ic" data-hue="${esc(it.hue || "core")}">${it.icon}</span>`;
      html += `<div class="pal-item${n === active ? " on" : ""}" role="option" id="pal-${n}" data-n="${n}" aria-selected="${n === active}" style="animation-delay:${Math.min(n, 10) * 14}ms">${face}<span class="grow"><strong>${highlight(it.label, it.marks)}</strong><small>${esc(it.hint || "")}</small></span><span class="go">${ICON_ENTER}</span></div>`;
    });
    palList.innerHTML = html;
    palQ.setAttribute("aria-activedescendant", "pal-" + active);
  };
  const ICON_ENTER = '<svg class="icon" viewBox="0 0 16 16" width="16" height="16" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round"><path d="M3 8h9.5M9 4.5 12.5 8 9 11.5"/></svg>';
  const move = (n) => {
    if (!shown.length) return;
    active = (n + shown.length) % shown.length;
    $$(".pal-item", palList).forEach((el) => {
      const on = Number(el.dataset.n) === active;
      el.classList.toggle("on", on);
      el.setAttribute("aria-selected", String(on));
      if (on) el.scrollIntoView({ block: "nearest" });
    });
    palQ.setAttribute("aria-activedescendant", "pal-" + active);
  };
  const go = (it) => {
    if (!it) return;
    if (it.run) return it.run();
    store.set(RECENT, [it.href, ...store.get(RECENT, []).filter((h) => h !== it.href)].slice(0, 6));
    location.href = it.href;
  };
  const openPal = () => {
    if (!pal) return;
    drawer(false);
    lastFocus = document.activeElement;
    items = collect();
    palQ.value = "";
    active = 0;
    pal.hidden = false;
    pal.classList.add("open");
    render();
    requestAnimationFrame(() => palQ.focus());
  };
  const closePal = () => {
    if (!pal || pal.hidden) return;
    pal.classList.remove("open");
    pal.hidden = true;
    lastFocus?.focus?.();
  };
  if (pal) {
    palQ.addEventListener("input", () => { active = 0; render(); });
    palQ.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown") { e.preventDefault(); move(active + 1); }
      else if (e.key === "ArrowUp") { e.preventDefault(); move(active - 1); }
      else if (e.key === "Enter") { e.preventDefault(); go(shown[active]); }
      else if (e.key === "Escape") { e.preventDefault(); closePal(); }
      else if (e.key === "Tab") { e.preventDefault(); move(active + (e.shiftKey ? -1 : 1)); }
    });
    palList.addEventListener("pointermove", (e) => {
      const el = e.target.closest(".pal-item");
      if (el && Number(el.dataset.n) !== active) move(Number(el.dataset.n));
    });
    palList.addEventListener("click", (e) => { const el = e.target.closest(".pal-item"); if (el) go(shown[Number(el.dataset.n)]); });
    pal.addEventListener("click", (e) => { if (e.target === pal) closePal(); });
  }
  const mac = /Mac|iPhone|iPad/.test(navigator.platform);
  $$(".mac-only").forEach((k) => { k.textContent = mac ? "⌘K" : "Ctrl K"; });

  // ---------------------------------------------------------------- tooltips
  const tip = document.createElement("div");
  tip.className = "tip";
  tip.setAttribute("role", "tooltip");
  document.body.append(tip);
  let tipFor = null;
  const showTip = (el, html, x, y, side) => {
    tipFor = el;
    tip.innerHTML = html;
    tip.style.left = x + "px";
    tip.style.top = y + "px";
    tip.style.transform = side === "right" ? "translate(14px, -50%)" : "";
    tip.classList.add("show");
  };
  const hideTip = () => { tipFor = null; tip.classList.remove("show"); };
  const tipHtml = (text) => {
    const i = text.indexOf(": ");
    return i > 0 ? `<b>${esc(text.slice(i + 2))}</b>${esc(text.slice(0, i))}` : esc(text);
  };
  document.addEventListener("pointerover", (e) => {
    const el = e.target.closest?.("[data-tip]");
    if (!el || el === tipFor) return;
    const r = el.getBoundingClientRect();
    if (el.closest(".guilds") && innerWidth > 920) showTip(el, esc(el.dataset.tip), r.right, r.top + r.height / 2, "right");
    else showTip(el, tipHtml(el.dataset.tip), r.left + r.width / 2, r.top);
  });
  document.addEventListener("pointerout", (e) => {
    const el = e.target.closest?.("[data-tip]");
    if (el && !el.contains(e.relatedTarget)) hideTip();
  });
  addEventListener("scroll", hideTip, { passive: true });

  // ---------------------------------------------------------------- area chart cursor
  $$("figure.area[data-points]").forEach((fig) => {
    let pts;
    try { pts = JSON.parse(fig.dataset.points); } catch (_) { return; }
    const plot = $(".area-plot", fig), cur = $(".area-cursor", fig), dot = $(".area-dot", fig);
    const unit = fig.dataset.unit || "";
    if (!plot || !pts.length) return;
    const at = (clientX) => {
      const r = plot.getBoundingClientRect();
      const x = ((clientX - r.left) / r.width) * 100;
      let i = 0;
      pts.forEach((p, n) => { if (Math.abs(p[0] - x) < Math.abs(pts[i][0] - x)) i = n; });
      const p = pts[i];
      plot.classList.add("hovering");
      cur.style.left = p[0] + "%";
      dot.style.left = p[0] + "%";
      dot.style.top = p[1] + "%";
      showTip(plot, `<b>${esc(String(p[3]))} ${esc(unit)}</b>${esc(i === pts.length - 1 ? "Today" : p[2])}`, r.left + (p[0] / 100) * r.width, r.top + (p[1] / 100) * r.height - 4);
    };
    plot.addEventListener("pointermove", (e) => at(e.clientX));
    plot.addEventListener("pointerleave", () => { plot.classList.remove("hovering"); hideTip(); });
  });

  // ---------------------------------------------------------------- numbers count up
  const fmt = new Intl.NumberFormat("en-US");
  const countUp = (el) => {
    // Only a plain figure with a short unit ("1,284", "66 h", "+42", "7%"), never a time or date.
    const m = el.textContent.trim().match(/^([+\-]?)(\d[\d,]*)(\s?[A-Za-z%]{0,5})$/);
    if (!m || calm) { el.classList.add("counted"); return; }
    const [, pre, num, post] = m;
    const target = Number(num.replace(/,/g, ""));
    if (!Number.isFinite(target) || Math.abs(target) < 2) { el.classList.add("counted"); return; }
    const t0 = performance.now(), dur = 900 + Math.min(600, Math.log10(Math.abs(target) + 1) * 160);
    el.classList.add("counted");
    const tick = (t) => {
      const k = Math.min(1, (t - t0) / dur);
      const e = k === 1 ? 1 : 1 - Math.pow(2, -10 * k);
      el.textContent = pre + fmt.format(Math.round(target * e)) + post;
      if (k < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  };
  const counters = $$("[data-count]");
  if (counters.length) {
    const io = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { io.unobserve(e.target); countUp(e.target); } }), { threshold: 0.4 });
    counters.forEach((el) => io.observe(el));
  }

  // ---------------------------------------------------------------- tabs that scroll on a phone
  // Fade the edge while more tabs sit off to the right.
  $$(".tabs").forEach((t) => {
    const edge = () => t.classList.toggle("more", t.scrollWidth - t.clientWidth - t.scrollLeft > 4);
    edge();
    t.addEventListener("scroll", edge, { passive: true });
    addEventListener("resize", edge, { passive: true });
    const on = $(".tab.on", t);
    if (on && t.scrollWidth > t.clientWidth) t.scrollLeft = Math.max(0, on.offsetLeft - 24);
  });

  // ---------------------------------------------------------------- segmented choices glide
  $$(".seg").forEach((seg) => {
    const lit = () => $(":scope > .on", seg) || $(":scope > :has(input:checked)", seg);
    const first = lit();
    if (!first || calm) return;
    const ind = document.createElement("span");
    ind.className = "seg-ind";
    const place = (el) => { ind.style.left = el.offsetLeft + "px"; ind.style.width = el.offsetWidth + "px"; };
    seg.prepend(ind);
    seg.classList.add("glide");
    place(first);
    seg.addEventListener("click", (e) => { const el = e.target.closest(".seg > :not(.seg-ind)"); if (el) place(el); });
    seg.addEventListener("change", () => { const el = lit(); if (el) place(el); });
  });

  // ---------------------------------------------------------------- the server's colour
  // Sample the server icon and light the dashboard in its most vivid colour. Remembered per
  // server so the next page starts in it (see the early script in <head>).
  const icon = document.body.dataset.icon;
  if (icon && guild) {
    const run = () => {
      const img = new Image();
      img.crossOrigin = "anonymous";
      img.onload = () => {
        try {
          const c = document.createElement("canvas");
          c.width = c.height = 40;
          const g = c.getContext("2d", { willReadFrequently: true });
          g.drawImage(img, 0, 0, 40, 40);
          const d = g.getImageData(0, 0, 40, 40).data;
          const bins = Array.from({ length: 12 }, () => ({ w: 0, r: 0, g: 0, b: 0 }));
          let all = 0;
          for (let i = 0; i < d.length; i += 4) {
            const r = d[i] / 255, gg = d[i + 1] / 255, b = d[i + 2] / 255, a = d[i + 3] / 255;
            const mx = Math.max(r, gg, b), mn = Math.min(r, gg, b), l = (mx + mn) / 2, ch = mx - mn;
            if (a < 0.5 || ch < 0.12) continue;
            const s = ch / (1 - Math.abs(2 * l - 1) || 1);
            let h = mx === r ? ((gg - b) / ch) % 6 : mx === gg ? (b - r) / ch + 2 : (r - gg) / ch + 4;
            h = (h * 60 + 360) % 360;
            const w = s * s * (1 - Math.abs(l - 0.55) * 1.4) * a;
            if (w <= 0) continue;
            const bin = bins[Math.floor(h / 30) % 12];
            bin.w += w; bin.r += r * w; bin.g += gg * w; bin.b += b * w; all += w;
          }
          const top = bins.reduce((a, b) => (b.w > a.w ? b : a));
          if (all < 8 || top.w < 4) return;
          let [r, gg, b] = [top.r / top.w, top.g / top.w, top.b / top.w];
          // To HSL, clamped to a lit, saturated colour that reads on the dark ground.
          const mx = Math.max(r, gg, b), mn = Math.min(r, gg, b), ch = mx - mn;
          let h = mx === r ? ((gg - b) / ch) % 6 : mx === gg ? (b - r) / ch + 2 : (r - gg) / ch + 4;
          h = Math.round((h * 60 + 360) % 360);
          const accent = `hsl(${h} 82% 66%)`;
          root.style.setProperty("--accent", accent);
          try { localStorage.setItem("accent:" + guild, accent); } catch (_) {}
        } catch (_) { /* a cross-origin icon without CORS: keep the default colour */ }
      };
      img.src = icon;
    };
    (window.requestIdleCallback || setTimeout)(run);
  }

  // ---------------------------------------------------------------- hero parallax
  const heroImg = $(".ov-bg img, .ov-bg .art");
  if (heroImg && !calm) {
    let ticking = false;
    addEventListener("scroll", () => {
      if (ticking) return;
      ticking = true;
      requestAnimationFrame(() => {
        const y = Math.min(scrollY, 400);
        heroImg.style.translate = `0 ${y * 0.35}px`;
        ticking = false;
      });
    }, { passive: true });
  }

  // ---------------------------------------------------------------- banner upload
  const pick = $("#banner-file");
  if (pick) {
    pick.addEventListener("change", async () => {
      const file = pick.files?.[0];
      if (!file) return;
      if (!file.type.startsWith("image/")) return toast("Pick a PNG, JPEG or WebP image", true);
      const btn = $("[data-banner-pick]");
      btn?.classList.add("busy");
      try {
        // Resize in the browser: wide enough for a large screen, small enough to store.
        let body = file, type = file.type;
        if (file.type !== "image/gif") {
          const bmp = await createImageBitmap(file);
          const k = Math.min(1, 2000 / bmp.width, 1100 / bmp.height);
          const c = document.createElement("canvas");
          c.width = Math.round(bmp.width * k);
          c.height = Math.round(bmp.height * k);
          c.getContext("2d").drawImage(bmp, 0, 0, c.width, c.height);
          body = await new Promise((res) => c.toBlob(res, "image/webp", 0.86));
          type = "image/webp";
          if (!body) { body = await new Promise((res) => c.toBlob(res, "image/jpeg", 0.88)); type = "image/jpeg"; }
        }
        const r = await fetch(pick.dataset.action, { method: "POST", headers: { "content-type": type }, body, credentials: "same-origin" });
        if (!r.ok) throw new Error(r.status === 403 ? "Only admins can change the banner" : r.status === 413 ? "That image is too large" : "The upload didn't go through");
        location.href = location.pathname + "?saved=1";
      } catch (err) {
        btn?.classList.remove("busy");
        toast(err.message || "The upload didn't go through", true);
      }
    });
  }

  // ---------------------------------------------------------------- clicks
  const closeOpen = (except) => $$(".pop[open]").forEach((d) => { if (!d.contains(except)) d.removeAttribute("open"); });

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
    if (e.target.closest("[data-palette]")) { e.preventDefault(); openPal(); return; }
    if (e.target.closest("[data-banner-pick]")) { e.preventDefault(); pick?.click(); return; }
    const menu = e.target.closest(".menu-btn");
    if (menu) drawer(!document.body.classList.contains("drawer"));
    closeOpen(e.target);
  });

  // A submitted form's button spins until the next page arrives.
  document.addEventListener("submit", (e) => {
    if (e.defaultPrevented) return;
    const b = e.submitter;
    if (b && b.classList.contains("btn")) setTimeout(() => b.classList.add("busy"), 120);
  });
  addEventListener("pageshow", () => $$(".btn.busy").forEach((b) => b.classList.remove("busy")));

  document.addEventListener("keydown", (e) => {
    const typing = e.target.closest?.("input, textarea, select, [contenteditable]");
    if ((e.key === "k" || e.key === "K") && (e.metaKey || e.ctrlKey)) { e.preventDefault(); pal && !pal.hidden ? closePal() : openPal(); return; }
    if (e.key === "/" && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) { e.preventDefault(); openPal(); return; }
    if (e.key !== "Escape") return;
    if (document.body.classList.contains("drawer")) return drawer(false);
    const open = $(".pop[open]");
    if (!open) return;
    open.removeAttribute("open");
    $("summary", open)?.focus();
  });

  // Switches marked data-autosubmit save as soon as they're flipped.
  document.addEventListener("change", (e) => {
    const el = e.target.closest("[data-autosubmit]");
    if (el) el.form.requestSubmit();
  });
})();
