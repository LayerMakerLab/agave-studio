/* Agave Studio homepage motion. No dependencies. */
(() => {
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];

  requestAnimationFrame(() => requestAnimationFrame(() => document.body.classList.add("loaded")));

  /* ---------- Nav turns solid after the hero starts scrolling ---------- */
  const nav = $("#nav");
  const onNav = () => nav.classList.toggle("solid", scrollY > 20);
  addEventListener("scroll", onNav, { passive: true }); onNav();

  /* ---------- Reveal on view ---------- */
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
  }, { rootMargin: "0px 0px -10% 0px", threshold: 0.12 });
  $$(".reveal").forEach((el) => io.observe(el));
  $$(".clip").forEach((el, i) => { el.style.transitionDelay = `${i * 70}ms`; io.observe(el); });
  $$(".lb-grid .lb").forEach((el, i) => { el.style.transitionDelay = `${(i % 4) * 80}ms`; io.observe(el); });

  /* ---------- Bento spotlight follows the pointer ---------- */
  $$(".b").forEach((b) => b.addEventListener("pointermove", (e) => {
    const r = b.getBoundingClientRect();
    b.style.setProperty("--mx", `${e.clientX - r.left}px`);
    b.style.setProperty("--my", `${e.clientY - r.top}px`);
  }));

  /* ---------- Promo strip ---------- */
  const promo = $("#promo");
  try { if (localStorage.getItem("agave-promo") === "x") promo.classList.add("gone"); } catch {}
  $("#promoX").addEventListener("click", () => { promo.classList.add("gone"); try { localStorage.setItem("agave-promo", "x"); } catch {} });

  /* ---------- Lazy video: attach src on demand ---------- */
  const arm = (v) => { if (v.dataset.src && !v.src) { v.src = v.dataset.src; v.load(); } return v; };
  const play = (v) => { if (reduce) return; arm(v); const p = v.play(); if (p) p.catch(() => {}); };

  /* ---------- Top bar menus (hover on desktop, click anywhere) ---------- */
  const items = $$(".nav-item.has-menu");
  const closeAll = (except) => items.forEach((it) => { if (it !== except) { it.classList.remove("open"); $(".nav-btn", it).setAttribute("aria-expanded", "false"); $$("video", it).forEach((v) => v.pause()); } });
  items.forEach((it) => {
    const btn = $(".nav-btn", it); let t;
    const open = () => { clearTimeout(t); closeAll(it); it.classList.add("open"); btn.setAttribute("aria-expanded", "true"); $$("video", it).forEach(play); };
    const shut = () => { t = setTimeout(() => { it.classList.remove("open"); btn.setAttribute("aria-expanded", "false"); $$("video", it).forEach((v) => v.pause()); }, 180); };
    it.addEventListener("pointerenter", (e) => { if (e.pointerType === "mouse") open(); });
    it.addEventListener("pointerleave", (e) => { if (e.pointerType === "mouse") shut(); });
    // A mouse already opened the menu on hover, so its click must not toggle it shut. Touch and keyboard toggle.
    btn.addEventListener("click", (e) => (e.pointerType === "mouse" || !it.classList.contains("open") ? open() : shut()));
    it.addEventListener("focusout", (e) => { if (!it.contains(e.relatedTarget)) shut(); });
  });
  addEventListener("keydown", (e) => { if (e.key === "Escape") closeAll(); });
  document.addEventListener("click", (e) => { if (!e.target.closest(".nav-item")) closeAll(); });

  /* ---------- Mobile menu ---------- */
  const burger = $("#burger"), mnav = $("#mnav");
  burger.addEventListener("click", () => { document.documentElement.style.setProperty("--navb", nav.getBoundingClientRect().bottom + "px"); const o = mnav.hidden; mnav.hidden = !o; burger.setAttribute("aria-expanded", String(o)); document.body.style.overflow = o ? "hidden" : ""; });
  $$("a, button", mnav).forEach((a) => a.addEventListener("click", () => { mnav.hidden = true; burger.setAttribute("aria-expanded", "false"); document.body.style.overflow = ""; }));

  /* ---------- Hero carousel: blur crossfade + progress dots ---------- */
  const hero = $(".hero"), slides = $$(".slide"), dotsEl = $("#dots"), titleEl = $("#heroTitle"), subEl = $("#heroSub"), tagEl = $("#heroTag");
  const DUR = 5200; let cur = 0, timer = null, first = true;
  hero.style.setProperty("--dur", DUR + "ms");
  const renderTitle = (raw) => {
    const [a, b, c] = raw.split("|");
    titleEl.innerHTML = "";
    const line = (parts) => { const l = document.createElement("span"); l.className = "line";
      parts.forEach(([txt, serif], i) => { const w = document.createElement("span"); w.className = "word" + (serif ? " serif" : ""); w.textContent = txt; w.style.transitionDelay = `${i * 90 + (l === null ? 0 : 0)}ms`; l.append(w, document.createTextNode(" ")); });
      titleEl.append(l); };
    line([[a, false]]); line([[b, true], [c, false]]);
    $$(".line:nth-child(2) .word", titleEl).forEach((w, i) => (w.style.transitionDelay = `${120 + i * 90}ms`));
    requestAnimationFrame(() => requestAnimationFrame(() => titleEl.classList.add("in")));
  };
  const dots = slides.map((_, i) => {
    const d = document.createElement("button"); d.className = "dot-b"; d.type = "button"; d.setAttribute("role", "tab");
    d.setAttribute("aria-label", `Shot ${i + 1}`); d.innerHTML = "<i></i>"; d.addEventListener("click", () => go(i, true)); dotsEl.append(d); return d;
  });
  const go = (n, user) => {
    const prev = slides[cur]; n = (n + slides.length) % slides.length;
    if (n !== cur) { prev.classList.remove("is-on"); prev.classList.add("is-out"); setTimeout(() => { prev.classList.remove("is-out"); $("video", prev).pause(); }, 1300); }
    cur = n; const s = slides[n], v = $("video", s);
    arm(v); try { v.currentTime = 0; } catch {} play(v); s.classList.add("is-on");
    arm($("video", slides[(n + 1) % slides.length])); // warm the next clip
    if (!first) {
      titleEl.classList.remove("in"); titleEl.classList.add("out"); subEl.classList.add("swap");
      setTimeout(() => { titleEl.classList.remove("out"); renderTitle(s.dataset.title); subEl.textContent = s.dataset.sub; tagEl.textContent = s.dataset.tag; subEl.classList.remove("swap"); }, 420);
    }
    first = false;
    dots.forEach((d, i) => { d.classList.remove("on"); d.setAttribute("aria-selected", String(i === n)); void d.offsetWidth; if (i === n) d.classList.add("on"); });
    clearTimeout(timer); if (!reduce) timer = setTimeout(() => go(cur + 1), DUR);
  };
  renderTitle(slides[0].dataset.title); subEl.textContent = slides[0].dataset.sub;
  go(0);
  // pause the carousel when the hero is off screen or the tab is hidden
  new IntersectionObserver(([e]) => {
    const v = $("video", slides[cur]);
    if (e.isIntersecting) { hero.classList.remove("paused"); play(v); if (!timer && !reduce) timer = setTimeout(() => go(cur + 1), DUR); }
    else { hero.classList.add("paused"); v.pause(); clearTimeout(timer); timer = null; }
  }, { threshold: 0.2 }).observe(hero);
  document.addEventListener("visibilitychange", () => { if (document.hidden) { clearTimeout(timer); timer = null; } else if (!timer && !reduce) timer = setTimeout(() => go(cur + 1), DUR); });
  hero.addEventListener("keydown", (e) => { if (e.key === "ArrowRight") go(cur + 1, true); if (e.key === "ArrowLeft") go(cur - 1, true); });

  /* ---------- Hover-to-play clips (cards, look book); in-view autoplay on touch ---------- */
  const touch = matchMedia("(hover: none)").matches;
  $$("[data-clip]").forEach((host) => {
    const v = $("video", host); if (!v) return;
    if (touch) new IntersectionObserver(([e]) => (e.isIntersecting ? play(v) : v.pause()), { threshold: 0.6 }).observe(host);
    else { host.addEventListener("pointerenter", () => { host.classList.add("playing"); play(v); }); host.addEventListener("pointerleave", () => { host.classList.remove("playing"); v.pause(); }); }
  });

  /* ---------- Before / after compare ---------- */
  $$(".compare").forEach((c) => {
    const set = (x) => { const r = c.getBoundingClientRect(); const p = Math.min(1, Math.max(0, (x - r.left) / r.width)); c.style.setProperty("--p", (p * 100).toFixed(2) + "%"); };
    let drag = false;
    c.addEventListener("pointerdown", (e) => { drag = true; c.setPointerCapture(e.pointerId); set(e.clientX); });
    c.addEventListener("pointermove", (e) => { if (drag || e.pointerType === "mouse") set(e.clientX); });
    c.addEventListener("pointerup", () => (drag = false));
    const range = $("input", c); if (range) range.addEventListener("input", () => c.style.setProperty("--p", range.value + "%"));
  });

  /* ---------- Prompt bar: types prompts, fakes nothing beyond a UI loop ---------- */
  const prompts = [
    ["A lighthouse keeper climbs the stairs during a storm, lantern swinging", "Video", "Dolly in"],
    ["Chrome sneaker floating over wet asphalt, neon reflections", "Image", "Low orbit"],
    ["Desert road at golden hour, car passes, dust hangs in the light", "Video", "Crane up"],
    ["Portrait of a jazz singer, warm stage light, shallow focus", "Image", "Rack focus"],
    ["Skateboarder kickflips down city stairs in slow motion", "Video", "Bullet time"],
  ];
  const pt = $("#promptText"), bar = $("#promptBar"), st = $("#promptState"), cMode = $("#chipMode"), cCam = $("#chipCam");
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const flash = (el, txt) => { el.textContent = txt; el.classList.add("flash"); setTimeout(() => el.classList.remove("flash"), 700); };
  (async () => {
    if (reduce) { pt.textContent = prompts[0][0]; return; }
    for (let i = 0; ; i = (i + 1) % prompts.length) {
      const [text, mode, cam] = prompts[i];
      flash(cMode, mode); flash(cCam, cam);
      st.textContent = "Writing"; bar.style.transition = "none"; bar.style.width = "0";
      for (let k = 1; k <= text.length; k++) { pt.textContent = text.slice(0, k); await sleep(28 + Math.random() * 40); }
      await sleep(500);
      st.textContent = "Rendering"; void bar.offsetWidth;
      bar.style.transition = "width 2.6s cubic-bezier(.65,0,.35,1)"; bar.style.width = "100%";
      await sleep(2700);
      st.textContent = "Done"; await sleep(1300);
      for (let k = text.length; k >= 0; k -= 3) { pt.textContent = text.slice(0, k); await sleep(12); }
    }
  })();

  /* ---------- Camera stage: scroll-driven camera moves ---------- */
  const cam = $("#camera"), world = $("#world"), moves = $$("#moves li"), sbar = $("#stageBar"), tc = $("#tc"), lens = $("#lens");
  const lenses = ["35mm · f/1.8", "50mm · f/2.0", "24mm · f/2.8", "85mm · f/1.4"];
  const ease = (x) => x < .5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2;
  const lerp = (a, b, k) => a + (b - a) * k;
  // Keyframes per move: [tx%, ty%, scale, rotX, rotY, rotZ] applied to a single plate
  const K = [
    [0, 0, 1.00, 0, 0, 0],      // start wide
    [0, 2, 1.38, 0, 0, 0],      // dolly in
    [-6, 1, 1.22, 0, 14, 0],    // orbit
    [-3, 9, 1.18, -10, 8, 0],   // crane up
    [7, 2, 1.26, 0, -10, 2],    // whip pan
  ];
  let active = -1, ticking = false;
  const camFrame = () => {
    ticking = false;
    const r = cam.getBoundingClientRect();
    const total = r.height - innerHeight;
    const p = Math.min(1, Math.max(0, -r.top / total));
    sbar.style.width = `${p * 100}%`;
    const seg = Math.min(3, Math.floor(p * 4));
    const local = ease(Math.min(1, (p * 4 - seg)));
    const a = K[seg], b = K[seg + 1];
    const v = a.map((x, i) => lerp(x, b[i], local));
    const shake = seg === 3 ? Math.sin(local * Math.PI) * 4 : 0; // whip pan motion blur feel
    world.style.transform = `translate3d(${v[0] + shake}%, ${v[1]}%, 0) scale(${v[2]}) rotateX(${v[3]}deg) rotateY(${v[4]}deg) rotateZ(${v[5]}deg)`;
    world.style.filter = seg === 3 ? `blur(${Math.sin(local * Math.PI) * 3}px)` : "none";
    if (seg !== active) {
      active = seg; moves.forEach((li, i) => li.classList.toggle("on", i === seg)); lens.textContent = lenses[seg];
    }
    const frames = Math.floor(p * 24 * 16);
    const ff = String(frames % 24).padStart(2, "0"), ss = String(Math.floor(frames / 24) % 60).padStart(2, "0");
    tc.textContent = `00:00:${ss}:${ff}`;
  };
  addEventListener("scroll", () => { if (!ticking) { ticking = true; requestAnimationFrame(camFrame); } }, { passive: true });
  addEventListener("resize", camFrame); camFrame();
})();
