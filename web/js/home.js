/* Agave Studio homepage. No dependencies and no requests of its own beyond the page's media. */
(() => {
  const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];

  // A video gets its file only when it is needed (data-src), and plays only while it is on screen.
  const arm = (v) => { if (v.dataset.src && !v.getAttribute("src")) { v.preload = "auto"; v.src = v.dataset.src; } return v; };
  const play = (v) => { if (reduce) return; arm(v); const p = v.play(); if (p) p.catch(() => {}); };

  /* ---------- Phone menu ---------- */
  const burger = $("#burger"), sheet = $("#sheet");
  const setSheet = (open) => {
    sheet.hidden = !open;
    burger.setAttribute("aria-expanded", String(open));
    document.body.classList.toggle("locked", open);
  };
  burger.addEventListener("click", () => setSheet(sheet.hidden));
  $$("a", sheet).forEach((a) => a.addEventListener("click", () => setSheet(false)));
  addEventListener("keydown", (e) => { if (e.key === "Escape" && !sheet.hidden) { setSheet(false); burger.focus(); } });
  matchMedia("(min-width: 961px)").addEventListener("change", (e) => { if (e.matches) setSheet(false); });

  /* ---------- Hero reel ----------
     Wide screens: one card is open and plays. The reel cuts to the next card half a second before the clip ends,
     while the shot is still moving. Pointing at a card opens it and holds it. Phones: the cards scroll sideways
     and the one in view plays. */
  const reel = $("#reel"), shots = $$(".shot", reel);
  const vids = shots.map((s) => $("video", s)), bars = shots.map((s) => $(".shot-bar", s));
  const narrow = matchMedia("(max-width: 760px)");
  const CUT = 0.5;
  let cur = 0, holding = false, visible = true, started = false;

  const stopBars = () => bars.forEach((b) => b.classList.remove("run"));
  const runBar = (i) => {
    const v = vids[i], bar = bars[i];
    bar.classList.remove("run");
    if (reduce || narrow.matches || holding || !v.duration || v.paused) return;
    const span = v.duration - CUT;
    bar.style.setProperty("--from", String(Math.min(1, v.currentTime / span)));
    bar.style.setProperty("--t", Math.max(0.1, span - v.currentTime) + "s");
    void bar.offsetWidth;
    bar.classList.add("run");
  };
  const open = (i) => {
    if (i === cur && shots[i].classList.contains("on") && started) return;
    started = true;
    shots.forEach((s, k) => s.classList.toggle("on", k === i));
    vids.forEach((v, k) => {
      if (k === i) return;
      v.pause(); bars[k].classList.remove("run");
      try { if (v.readyState) v.currentTime = 0; } catch {}
    });
    cur = i;
    if (visible) play(vids[i]);
  };

  vids.forEach((v, i) => {
    v.addEventListener("playing", () => {
      if (i !== cur) return;
      runBar(i);
      if (!reduce) arm(vids[(i + 1) % vids.length]); // warm the next cut once this one plays
    });
    v.addEventListener("pause", () => { if (i === cur) bars[i].classList.remove("run"); });
    v.addEventListener("timeupdate", () => {
      if (i !== cur || holding || narrow.matches || reduce || !v.duration) return;
      if (v.currentTime >= v.duration - CUT) open((i + 1) % vids.length);
    });
    v.addEventListener("ended", () => { if (i === cur) { v.currentTime = 0; play(v); } }); // held, or on a phone: loop
  });

  shots.forEach((s, i) => {
    s.addEventListener("pointerenter", (e) => { if (e.pointerType !== "mouse" || narrow.matches) return; holding = true; stopBars(); open(i); });
    s.addEventListener("focusin", () => { if (narrow.matches) return; holding = true; stopBars(); open(i); });
  });
  reel.addEventListener("pointerleave", (e) => { if (e.pointerType !== "mouse") return; holding = false; runBar(cur); });
  reel.addEventListener("focusout", (e) => { if (!reel.contains(e.relatedTarget)) { holding = false; runBar(cur); } });

  if ("IntersectionObserver" in window) {
    new IntersectionObserver(([e]) => {
      visible = e.isIntersecting;
      if (visible) play(vids[cur]); else vids[cur].pause();
    }, { threshold: 0.15 }).observe(reel);
    // Phones: the card that is mostly in view plays; the rest wait on their first frame.
    const side = new IntersectionObserver((es) => {
      if (!narrow.matches) return;
      es.forEach((e) => {
        const i = shots.indexOf(e.target);
        if (e.intersectionRatio >= 0.6) { cur = i; shots.forEach((s, k) => s.classList.toggle("on", k === i)); if (visible) play(vids[i]); }
        else vids[i].pause();
      });
    }, { root: reel, threshold: [0, 0.6] });
    shots.forEach((s) => side.observe(s));
  }
  document.addEventListener("visibilitychange", () => { if (document.hidden) vids[cur].pause(); else if (visible) play(vids[cur]); });
  narrow.addEventListener("change", () => { holding = false; runBar(cur); });
  // The first clip loads after the page itself, so posters and type come first.
  const begin = () => { if (!narrow.matches) open(0); };
  if (document.readyState === "complete") begin(); else addEventListener("load", begin, { once: true });

  /* ---------- Reel and pipeline clips: load near the screen, play only on it ---------- */
  const clips = $$(".masonry video, .track video, .vis-clip video");
  if (!reduce && "IntersectionObserver" in window) {
    const near = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { arm(e.target); near.unobserve(e.target); } }), { rootMargin: "300px 0px" });
    const seen = new IntersectionObserver((es) => es.forEach((e) => (e.isIntersecting ? play(e.target) : e.target.pause())), { threshold: 0.2 });
    clips.forEach((v) => { near.observe(v); seen.observe(v); });
  }

  /* ---------- Reel: filter, and the fold that "See all" opens ---------- */
  const filters = $$("#gallery .filters button"), items = $$("#masonry .m"), masonry = $("#masonry");
  const wrap = $("#reelWrap"), seeAll = $("#seeAll");
  const setOpen = (open) => { wrap.classList.toggle("open", open); seeAll.setAttribute("aria-expanded", String(open)); seeAll.textContent = open ? "Show less" : "See all"; };
  seeAll.addEventListener("click", () => {
    const open = !wrap.classList.contains("open");
    setOpen(open);
    if (!open) $("#gallery").scrollIntoView({ block: "start" });
  });
  filters.forEach((b) => b.addEventListener("click", () => {
    const k = b.dataset.show;
    filters.forEach((x) => { const on = x === b; x.classList.toggle("on", on); x.setAttribute("aria-pressed", String(on)); });
    masonry.classList.toggle("filtered", k !== "all");
    if (k !== "all") setOpen(true);
    items.forEach((m) => {
      const hide = k !== "all" && !m.classList.contains(k);
      m.hidden = hide;
      if (hide) { const v = $("video", m); if (v) v.pause(); }
    });
  }));

  /* ---------- Pipelines: one lane at a time ---------- */
  const laneTabs = $$("#pipelines [data-lane]");
  laneTabs.forEach((t) => t.addEventListener("click", () => {
    laneTabs.forEach((x) => {
      const on = x === t, lane = $("#lane-" + x.dataset.lane);
      x.classList.toggle("on", on); x.setAttribute("aria-selected", String(on));
      lane.hidden = !on;
      if (!on) $$("video", lane).forEach((v) => v.pause());
    });
  }));

  /* ---------- Blocks below the fold wipe up once, left to right in each row ---------- */
  if (!reduce && "IntersectionObserver" in window) {
    const io = new IntersectionObserver((es) => es.forEach((e) => {
      if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
    }), { rootMargin: "0px 0px -8% 0px" });
    $$("[data-wipe]").forEach((el) => {
      if (el.getBoundingClientRect().top < innerHeight) return;
      const row = [...el.parentElement.children].filter((c) => c.hasAttribute("data-wipe"));
      el.style.setProperty("--d", (row.indexOf(el) % 4) * 60 + "ms");
      el.classList.add("pre");
      io.observe(el);
    });
  }
})();
