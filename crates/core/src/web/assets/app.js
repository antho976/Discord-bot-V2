// The dashboard's one script. Everything works without it; it adds the search palette, the
// server's colour, chart cursors and tooltips, numbers that count up, the phone drawer, the
// banner upload, button states, toasts, two-click deletes and the timezone cookie.
//
// Links and forms inside the dashboard change only the page (`visit`): the sidebar, the server
// rail and their scroll stay as they are, and the current page's pill glides to its new place.
// Setup that belongs to a page runs in `mount`, once on load and again after every swap.
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

  // A page's lifetime. Feature scripts that set timers or listen on the whole document pass
  // `window.botPage.signal` (or check `.aborted`), so they stop when the page is swapped out.
  let life = new AbortController();
  window.botPage = { signal: life.signal };

  // ---------------------------------------------------------------- toast
  const toast = (text, bad = false) => {
    const t = document.createElement("div");
    t.className = "toast" + (bad ? " bad" : "");
    t.setAttribute("role", "status");
    t.innerHTML = `<span class="toast-ic">${bad ? ICON_BAD : ICON_OK}</span><span></span>`;
    t.lastChild.textContent = text;
    document.body.append(t);
    requestAnimationFrame(() => requestAnimationFrame(() => t.classList.add("show")));
    setTimeout(() => { t.classList.remove("show"); setTimeout(() => t.remove(), 400); }, 2600);
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

  // ---------------------------------------------------------------- sidebar
  // Longest matching link wins, so /g/1/tickets/starboard marks Starboard, not Tickets. The
  // pill keeps one view-transition-name, so moving it inside a transition makes it glide.
  const markNav = () => {
    let best = null;
    $$(".side-links .nav").forEach((a) => {
      const p = a.dataset.path || new URL(a.href).pathname;
      const hit = location.pathname === p || (a.dataset.path && location.pathname.startsWith(p + "/"));
      if (hit && (!best || p.length > (best.dataset.path || new URL(best.href).pathname).length)) best = a;
    });
    const was = $(".side-links .nav.on");
    if (best === was) return best;
    const pill = $(".side-links .nav-pill") || Object.assign(document.createElement("span"), { className: "nav-pill" });
    $$(".side-links .nav.on").forEach((a) => { a.classList.remove("on"); a.removeAttribute("aria-current"); });
    if (best) {
      best.classList.add("on");
      best.setAttribute("aria-current", "page");
      best.prepend(pill);
      best.closest(".nav-fold")?.setAttribute("open", "");
    } else pill.remove();
    return best;
  };
  const current = markNav();
  // Keep the current page in view in a long list, without moving a list the person scrolled.
  const list = $(".side-links");
  const SCROLL = "side-scroll";
  if (list) {
    let saved = null;
    try { saved = sessionStorage.getItem(SCROLL); } catch (_) {}
    if (saved !== null) list.scrollTop = Number(saved);
    else if (current) {
      const r = current.getBoundingClientRect(), lr = list.getBoundingClientRect();
      if (r.bottom > lr.bottom - 20) list.scrollTop += r.bottom - lr.bottom + 60;
    }
    list.addEventListener("scroll", () => { try { sessionStorage.setItem(SCROLL, String(list.scrollTop)); } catch (_) {} }, { passive: true });
  }

  // Groups stay as the person folded them, per browser. The current page's group stays open.
  // Opening one lets its pages rise in; nothing animates on a page load.
  const CLOSED = "nav-closed";
  let closed = store.get(CLOSED, []);
  $$(".nav-fold").forEach((d) => {
    const g = d.dataset.group;
    if (closed.includes(g) && !d.contains(current)) d.removeAttribute("open");
    d.addEventListener("toggle", () => {
      closed = closed.filter((x) => x !== g);
      if (!d.open) closed.push(g);
      store.set(CLOSED, closed);
      if (d.open && !calm) { d.classList.add("opening"); setTimeout(() => d.classList.remove("opening"), 500); }
    });
  });

  // Counts in the sidebar and on the bell, brought up to date from a fetched page.
  const syncCounts = (doc) => {
    $$(".side-links .nav").forEach((a) => {
      const twin = doc.querySelector(`.side-links .nav[href="${a.getAttribute("href")}"]`);
      const now = $(".nav-count", a), next = twin && twin.querySelector(".nav-count");
      if (now && !next) { now.classList.add("leaving"); setTimeout(() => now.remove(), 250); }
      else if (next && !now) a.append(next.cloneNode(true));
      else if (now && next && now.textContent !== next.textContent) now.replaceWith(next.cloneNode(true));
    });
    $$(".nav-fold").forEach((d) => {
      const twin = doc.querySelector(`.nav-fold[data-group="${d.dataset.group}"] > summary .nav-count`);
      const now = $(":scope > summary .nav-count", d);
      if (now && !twin) now.remove();
      else if (twin && !now) $(":scope > summary .icon", d)?.before(twin.cloneNode(true));
      else if (now && twin) now.replaceWith(twin.cloneNode(true));
    });
    $$(".bell").forEach((b) => {
      const twin = doc.querySelector(".page-corner .bell, .topbar .bell");
      const now = $(".count", b), next = twin && twin.querySelector(".count");
      if (now && !next) now.remove();
      else if (next) { now ? now.replaceWith(next.cloneNode(true)) : b.append(next.cloneNode(true)); }
    });
  };

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
    visit(it.href);
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
  // On a big screen the page is zoomed (app.css), so a fixed position of 100px lands further
  // right than 100 screen pixels. Measure how far, once per window size.
  const probe = Object.assign(document.createElement("i"), { ariaHidden: "true" });
  probe.style.cssText = "position:fixed;left:100px;top:0;width:0;height:0;visibility:hidden;pointer-events:none";
  document.body.append(probe);
  let scale = 0;
  addEventListener("resize", () => { scale = 0; }, { passive: true });
  const zf = () => { if (!scale) scale = probe.getBoundingClientRect().left / 100 || 1; return scale; };
  const px = (v) => v / zf() + "px";
  const showTip = (el, html, x, y, side) => {
    tipFor = el;
    tip.innerHTML = html;
    tip.style.left = px(x);
    tip.style.top = px(y);
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
  const areaCursor = (fig) => {
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
  };

  // ---------------------------------------------------------------- numbers count up
  const fmt = new Intl.NumberFormat("en-US");
  const countUp = (el) => {
    // Only a plain figure with a short unit ("1,284", "66 h", "+42", "7%"), never a time or date.
    const m = el.textContent.trim().match(/^([+\-]?)(\d[\d,]*)(\s?[A-Za-z%]{0,5})$/);
    if (!m || calm || root.classList.contains("same")) { el.classList.add("counted"); return; }
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

  // ---------------------------------------------------------------- tabs, segmented choices
  const tabsFade = (t, signal) => {
    const edge = () => t.classList.toggle("more", t.scrollWidth - t.clientWidth - t.scrollLeft > 4);
    edge();
    t.addEventListener("scroll", edge, { passive: true });
    addEventListener("resize", edge, { passive: true, signal });
    const on = $(".tab.on", t);
    if (on && t.scrollWidth > t.clientWidth) t.scrollLeft = Math.max(0, on.offsetLeft - 24);
  };
  const segGlide = (seg) => {
    const lit = () => $(":scope > .on", seg) || $(":scope > :has(input:checked)", seg);
    const first = lit();
    if (!first || calm || seg.classList.contains("glide")) return;
    const ind = document.createElement("span");
    ind.className = "seg-ind";
    const place = (el) => { ind.style.left = el.offsetLeft + "px"; ind.style.width = el.offsetWidth + "px"; };
    seg.prepend(ind);
    seg.classList.add("glide");
    place(first);
    seg.addEventListener("click", (e) => { const el = e.target.closest(".seg > :not(.seg-ind)"); if (el) place(el); });
    seg.addEventListener("change", () => { const el = lit(); if (el) place(el); });
  };

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
  if (!calm) {
    let ticking = false;
    addEventListener("scroll", () => {
      if (ticking) return;
      ticking = true;
      requestAnimationFrame(() => {
        const hero = $(".ov-bg img, .ov-bg .art");
        if (hero) hero.style.translate = `0 ${Math.min(scrollY, 400) * 0.35}px`;
        ticking = false;
      });
    }, { passive: true });
  }

  // ---------------------------------------------------------------- banner upload
  document.addEventListener("change", async (e) => {
    const pick = e.target.closest?.("#banner-file");
    if (!pick) return;
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
      // The banner also heads the sidebar, so this one is a full load.
      location.href = location.pathname + "?saved=1";
    } catch (err) {
      btn?.classList.remove("busy");
      toast(err.message || "The upload didn't go through", true);
    }
  });

  // ---------------------------------------------------------------- unsaved changes
  // Save sits at the end of its form. Once something in a settings form changes and that
  // button is out of view, a bar like Discord's own rises at the bottom: Reset or Save.
  const bar = document.createElement("div");
  bar.className = "unsaved";
  bar.setAttribute("role", "status");
  bar.innerHTML = '<span>You have unsaved changes</span><button type="button" class="btn small ghost" data-unsaved="reset">Reset</button><button type="button" class="btn small primary" data-unsaved="save">Save changes</button>';
  document.body.append(bar);
  let dirty = null;
  const snap = (f) => { try { return new URLSearchParams(new FormData(f)).toString(); } catch (_) { return ""; } };
  const syncBar = () => bar.classList.toggle("show", !!dirty && !dirty.saveSeen && document.contains(dirty));
  const watchForms = (scope, signal) => $$("form.settings", scope).forEach((f) => {
    f.dataset.snap = snap(f);
    const btn = $(".save-bar .btn", f);
    if (!btn) return;
    const io = new IntersectionObserver(([en]) => { f.saveSeen = en.isIntersecting; syncBar(); });
    io.observe(btn);
    signal.addEventListener("abort", () => io.disconnect());
  });
  const edited = (e) => {
    const f = e.target.closest?.("form.settings");
    if (!f || f.dataset.snap === undefined) return;
    const changed = snap(f) !== f.dataset.snap;
    f.classList.toggle("dirty", changed);
    if (changed) dirty = f; else if (dirty === f) dirty = null;
    syncBar();
  };
  document.addEventListener("input", edited);
  document.addEventListener("change", edited);
  bar.addEventListener("click", (e) => {
    const b = e.target.closest("[data-unsaved]");
    if (!b || !dirty) return;
    if (b.dataset.unsaved === "save") return dirty.requestSubmit($(".save-bar .btn", dirty) || undefined);
    const f = dirty;
    f.reset();
    // A reset sets values after its event; re-read them on the next turn.
    setTimeout(() => { f.dispatchEvent(new Event("input", { bubbles: true })); f.dispatchEvent(new Event("change", { bubbles: true })); }, 0);
  });

  // Unsaved edits survive the page coming back to itself (a test button posts the form
  // elsewhere and returns): the values are put into the new form, which stays "unsaved".
  const carry = (from) => (from && document.contains(from) ? { action: from.getAttribute("action"), data: new FormData(from) } : null);
  const restore = (page, kept) => {
    const f = [...page.querySelectorAll("form.settings")].find((x) => x.getAttribute("action") === kept.action);
    if (!f) return;
    const seen = {};
    [...f.elements].forEach((el) => {
      if (!el.name || el.disabled || el.type === "file" || el.type === "submit" || el.type === "button") return;
      const all = kept.data.getAll(el.name).map(String);
      if (el.type === "checkbox" || el.type === "radio") el.checked = all.includes(el.value);
      else if (el.multiple) [...el.options].forEach((o) => { o.selected = all.includes(o.value); });
      else {
        const i = (seen[el.name] = (seen[el.name] ?? -1) + 1);
        if (all[i] !== undefined) el.value = all[i];
      }
    });
    f.dispatchEvent(new Event("input", { bubbles: true }));
    f.dispatchEvent(new Event("change", { bubbles: true }));
  };

  // ---------------------------------------------------------------- the page's own setup
  // Everything a page needs once it's on screen, on the first load and after each swap.
  const mount = (page) => {
    const signal = life.signal;
    const url = new URL(location.href);
    if (url.searchParams.has("saved")) {
      toast("Saved");
      url.searchParams.delete("saved");
      history.replaceState(history.state, "", url.pathname + url.search + url.hash);
    }
    // A test post's answer (`ui::sent_toast`): say it, then drop only the `sent` flag.
    const told = $("[data-toast]", page);
    if (told) {
      toast(told.dataset.toast);
      told.remove();
      const u = new URL(location.href);
      u.searchParams.delete("sent");
      history.replaceState(history.state, "", u.pathname + u.search + u.hash);
    }
    $$("figure.area[data-points]", page).forEach(areaCursor);
    const counters = $$("[data-count]", page);
    if (counters.length) {
      const io = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { io.unobserve(e.target); countUp(e.target); } }), { threshold: 0.4 });
      counters.forEach((el) => io.observe(el));
      signal.addEventListener("abort", () => io.disconnect());
    }
    $$(".tabs", page).forEach((t) => tabsFade(t, signal));
    $$(".seg", page).forEach(segGlide);
    watchForms(page, signal);
    $$(".mac-only", page).forEach((k) => { k.textContent = mac ? "⌘K" : "Ctrl K"; });
  };

  // A submitted form's button spins after a beat; a quick answer cancels the spin.
  let busyTimers = [];
  const unbusy = () => { busyTimers.forEach(clearTimeout); busyTimers = []; $$(".btn.busy").forEach((b) => b.classList.remove("busy")); };

  // ---------------------------------------------------------------- refusals
  // When the server turns an action down (a setting isn't set yet, a value is wrong), say why
  // in a small popup by the button that asked, instead of opening a page. It stays until it's
  // dismissed: OK, Escape, a click elsewhere, or leaving the page.
  const refusal = document.createElement("div");
  refusal.className = "refusal";
  refusal.setAttribute("role", "alertdialog");
  refusal.innerHTML = '<span class="refusal-ic"><svg class="icon" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round"><path d="M8 4.2v4.6M8 11.6v.01"/></svg></span><div class="refusal-text"><strong></strong><p></p></div><button type="button" class="btn small">OK</button>';
  document.body.append(refusal);
  let refusedFrom = null;
  const closeRefusal = () => {
    if (!refusal.classList.contains("show")) return false;
    refusal.classList.remove("show");
    if (refusedFrom && document.contains(refusedFrom)) refusedFrom.focus({ preventScroll: true });
    refusedFrom = null;
    return true;
  };
  const refuse = (title, text, from) => {
    $("strong", refusal).textContent = title;
    $("p", refusal).textContent = text;
    refusal.classList.remove("show", "above");
    const k = zf();
    const w = refusal.offsetWidth * k, h = refusal.offsetHeight * k;
    const r = from && document.contains(from) ? from.getBoundingClientRect() : null;
    const seen = r && r.bottom > 0 && r.top < innerHeight && r.width > 0;
    refusedFrom = seen ? from : null;
    let x = innerWidth / 2, y = 76;
    if (seen) {
      x = r.left + r.width / 2;
      y = r.bottom + 10;
      if (y + h > innerHeight - 12) { y = r.top - 10; refusal.classList.add("above"); }
    }
    x = Math.min(Math.max(x, w / 2 + 12), innerWidth - w / 2 - 12);
    refusal.style.left = px(x);
    refusal.style.top = px(y);
    requestAnimationFrame(() => { refusal.classList.add("show"); $("button", refusal).focus({ preventScroll: true }); });
  };
  refusal.addEventListener("click", (e) => { if (e.target.closest("button")) closeRefusal(); });
  document.addEventListener("pointerdown", (e) => { if (!refusal.contains(e.target)) closeRefusal(); });
  addEventListener("resize", closeRefusal, { passive: true });

  // ---------------------------------------------------------------- page swaps
  // A link or form inside the dashboard fetches its page and puts in the new <main> only.
  // Anything that isn't a dashboard page (sign-in, an error, another server, a file) falls
  // back to an ordinary visit. Same page again (a filter, a save): no slide, scroll kept.
  const boostable = (u) => u.origin === location.origin && /^\/(g\/\d+(\/|$)|users(\/|$)|activity$)/.test(u.pathname);
  let inflight = null;
  const run = (page) => {
    // Scripts that came with the page run now, in order, as they would have on a load.
    $$("script", page).forEach((old) => {
      const s = document.createElement("script");
      [...old.attributes].forEach((a) => s.setAttribute(a.name, a.value));
      s.textContent = old.textContent;
      old.replaceWith(s);
    });
  };
  const visit = async (href, opts = {}) => {
    const u = new URL(href, location.href);
    // Only from a dashboard page (not the public pets pages), to a page of this server.
    if (!$("main.page") || !boostable(u) || (u.pathname.startsWith("/g/") && !u.pathname.startsWith(`/g/${guild}`))) { location.href = u.href; return; }
    inflight?.abort();
    const ctl = (inflight = new AbortController());
    const main = $("main.page");
    const timer = setTimeout(() => root.classList.add("loading"), 180);
    try {
      const res = await fetch(u.href, { method: opts.method || "GET", body: opts.body, headers: opts.headers, credentials: "same-origin", signal: ctl.signal });
      const type = res.headers.get("content-type") || "";
      const posted = (opts.method || "GET") !== "GET";
      // A post that didn't go through says why by its button, whatever the status. Loading the
      // post's own address instead would ask for it with GET, which it doesn't answer.
      if (posted && !res.ok && !type.includes("text/html")) {
        unbusy();
        const text = (await res.text()).trim().slice(0, 300);
        refuse("That didn't go through", text || `The server answered ${res.status}.`, opts.from);
        return;
      }
      if (!type.includes("text/html")) { location.href = res.url; return; }
      const html = await res.text();
      const doc = new DOMParser().parseFromString(html, "text/html");
      const next = doc.querySelector("main.page");
      // A page built for another server (People and Activity use the first one): its sidebar
      // isn't this one, so load it whole.
      if (next && doc.body.dataset.guild !== guild) { location.href = res.url; return; }
      if (!next) {
        const why = doc.querySelector(".notice p")?.textContent;
        if (posted && (!res.ok || why)) {
          unbusy();
          refuse(doc.querySelector(".notice h1")?.textContent || "That didn't go through", why || `The server answered ${res.status}.`, opts.from);
          return;
        }
        location.href = res.url;
        return;
      }
      // A post answered with a page of its own (not a redirect) keeps the current address: the
      // post's address can't be loaded again with a reload.
      const stay = posted && !res.redirected;
      const to = stay ? new URL(location.href) : new URL(res.url);
      // fetch drops the fragment; a link's own (#pet-12) is put back and scrolled to.
      if (!posted && u.hash && to.pathname === u.pathname) to.hash = u.hash;
      const same = to.pathname === location.pathname;
      const swap = () => {
        closeRefusal();
        life.abort();
        life = new AbortController();
        window.botPage = { signal: life.signal };
        hideTip();
        drawer(false);
        $$(".pop[open]").forEach((d) => d.removeAttribute("open"));
        document.title = doc.title;
        if (opts.push !== false) {
          if (same) history.replaceState({ boost: 1 }, "", to.href);
          else history.pushState({ boost: 1 }, "", to.href);
        }
        root.classList.toggle("same", same);
        if (same || calm || !document.startViewTransition) root.classList.remove("vt");
        // Edits survive the page coming back to itself, except after that form's own save:
        // then the page shows what was saved.
        const kept = same && !opts.saving ? carry(dirty) : null;
        dirty = null;
        const fresh = document.importNode(next, true);
        main.replaceWith(fresh);
        markNav();
        syncCounts(doc);
        if (!same) window.scrollTo(0, opts.scroll || 0);
        mount(fresh);
        run(fresh);
        if (kept) restore(fresh, kept);
        syncBar();
        if (to.hash) document.getElementById(decodeURIComponent(to.hash.slice(1)))?.scrollIntoView({ block: "start" });
        if (!same) fresh.focus({ preventScroll: true });
      };
      unbusy();
      if (!same && !calm && document.startViewTransition) {
        // html.vt stays until the next swap: taking it off would restart the entrances.
        root.classList.add("vt");
        document.startViewTransition(swap);
      } else swap();
    } catch (err) {
      if (err.name !== "AbortError") location.href = u.href;
    } finally {
      clearTimeout(timer);
      root.classList.remove("loading");
      if (inflight === ctl) inflight = null;
    }
  };
  window.botVisit = visit;
  history.replaceState({ boost: 1 }, "", location.href);
  addEventListener("popstate", (e) => { if (e.state?.boost) visit(location.href, { push: false }); });

  // ---------------------------------------------------------------- in-place switches
  // A link marked data-swap (the overview's readings and its 7 / 30 days) changes one part of
  // the page: fetch where it points, put in the parts data-swap names, bring the other
  // data-swap links up to date, and leave the rest of the page still.
  let swapping = null;
  const partSwap = async (a) => {
    const parts = a.dataset.swap.split(/\s*,\s*/).map((s) => $(s));
    if (parts.some((p) => !p)) return visit(a.href);
    swapping?.abort();
    const ctl = (swapping = new AbortController());
    parts.forEach((p) => p.classList.add("swapping"));
    try {
      const res = await fetch(a.href, { credentials: "same-origin", signal: ctl.signal });
      if (!res.ok || res.redirected) throw new Error("visit");
      const doc = new DOMParser().parseFromString(await res.text(), "text/html");
      const next = a.dataset.swap.split(/\s*,\s*/).map((s) => doc.querySelector(s));
      const olds = $$("a[data-swap]"), news = [...doc.querySelectorAll("a[data-swap]")];
      if (next.some((n) => !n) || olds.length !== news.length) throw new Error("visit");
      const line = $(".stat-line"), from = line?.getBoundingClientRect();
      olds.forEach((o, i) => {
        const n = news[i];
        o.href = n.getAttribute("href");
        o.className = n.className;
        n.hasAttribute("aria-current") ? o.setAttribute("aria-current", n.getAttribute("aria-current")) : o.removeAttribute("aria-current");
        if (line && n.querySelector(".stat-line") && !o.contains(line)) o.append(line);
      });
      if (line && from && !calm) {
        const to = line.getBoundingClientRect();
        if (to.width) line.animate([{ transform: `translateX(${(from.left - to.left) / (to.width / line.offsetWidth)}px) scaleX(${from.width / to.width})`, transformOrigin: "left" }, { transform: "none", transformOrigin: "left" }], { duration: 420, easing: "cubic-bezier(0.16, 1, 0.3, 1)" });
      }
      parts.forEach((p, i) => {
        const n = document.importNode(next[i], true);
        p.replaceWith(n);
        $$("figure.area[data-points]", n).forEach(areaCursor);
        $$("[data-count]", n).forEach(countUp);
      });
      history.replaceState({ boost: 1 }, "", a.href);
    } catch (err) {
      if (err.name !== "AbortError") visit(a.href);
    } finally {
      if (swapping === ctl) swapping = null;
      parts.forEach((p) => p.classList.remove("swapping"));
    }
  };

  // ---------------------------------------------------------------- the bell
  // Opening it loads the newest notifications and marks them seen; "See all" is the page.
  document.addEventListener("toggle", async (e) => {
    const d = e.target;
    if (!d.matches?.("details[data-peek]") || !d.open) return;
    const body = $(".peek", d);
    try {
      const res = await fetch(d.dataset.peek, { credentials: "same-origin" });
      if (!res.ok || res.redirected) throw new Error();
      body.innerHTML = await res.text();
      $$(".bell .count").forEach((c) => { c.classList.add("leaving"); setTimeout(() => c.remove(), 250); });
    } catch (_) {
      body.innerHTML = '<p class="peek-none">Notifications didn\'t load. Open the page instead.</p>';
    }
  }, true);

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
    if (e.target.closest("[data-banner-pick]")) { e.preventDefault(); $("#banner-file")?.click(); return; }
    const menu = e.target.closest(".menu-btn");
    if (menu) drawer(!document.body.classList.contains("drawer"));
    closeOpen(e.target);
    // Links: a part swap, a page swap, or the browser's own way.
    const a = e.target.closest("a[href]");
    if (!a || e.defaultPrevented || e.button || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    if (a.target || a.hasAttribute("download") || a.hasAttribute("data-full") || a.getAttribute("href").startsWith("#")) return;
    if (a.dataset.swap) { e.preventDefault(); partSwap(a); return; }
    const u = new URL(a.href);
    if (!boostable(u)) return;
    e.preventDefault();
    visit(u.href);
  });

  // Forms inside the dashboard post in place: the answer replaces the page, a refusal shows
  // as a toast. The submitting button spins until then.
  document.addEventListener("submit", (e) => {
    if (e.defaultPrevented) return;
    const f = e.target;
    const b = e.submitter;
    if (b && b.classList.contains("btn")) busyTimers.push(setTimeout(() => b.classList.add("busy"), 120));
    // A real save of the changed form (not a test posting it elsewhere with formaction).
    // "Unsaved" clears when the save's answer arrives, not before: a refusal keeps it.
    const method = (b?.getAttribute("formmethod") || f.getAttribute("method") || "get").toUpperCase();
    const action = new URL(b?.getAttribute("formaction") || f.getAttribute("action") || location.href, location.href);
    if (f.target || f.hasAttribute("data-full") || !boostable(action) || f.closest(".account-menu")) return;
    e.preventDefault();
    const data = new FormData(f, b || undefined);
    // The button that asked, or the form's own Save, anchors a refusal popup.
    const from = b || $(".save-bar .btn", f) || f;
    const saving = f === dirty && !b?.hasAttribute("formaction");
    if (method === "GET") {
      action.search = new URLSearchParams(data).toString();
      visit(action.href, { from, saving });
    } else if ((f.enctype || "").includes("multipart")) {
      visit(action.href, { method, body: data, from, saving });
    } else {
      visit(action.href, { method, body: new URLSearchParams(data), headers: { "content-type": "application/x-www-form-urlencoded" }, from, saving });
    }
  });
  addEventListener("pageshow", () => $$(".btn.busy").forEach((b) => b.classList.remove("busy")));

  document.addEventListener("keydown", (e) => {
    const typing = e.target.closest?.("input, textarea, select, [contenteditable]");
    if ((e.key === "k" || e.key === "K") && (e.metaKey || e.ctrlKey)) { e.preventDefault(); pal && !pal.hidden ? closePal() : openPal(); return; }
    if (e.key === "/" && !typing && !e.ctrlKey && !e.metaKey && !e.altKey) { e.preventDefault(); openPal(); return; }
    if (e.key !== "Escape") return;
    if (closeRefusal()) return;
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

  mount($("main.page") || document.body);
})();
