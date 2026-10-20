/* Agave Studio Playground: any fal model with its own form.
   The gateway compiles each model's fal schema into fields (/api/fal/model); this page draws them, sends only what
   you set (fal's defaults cover the rest), shows fal's price before Run, and follows the run in the job queue.
   Everything from fal (names, descriptions, errors) is set as text, never as HTML. */
(() => {
  'use strict';

  const DEFAULT_MODEL = 'fal-ai/flux/schnell';
  const MAC = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
  const MAX_FILE = 20 * 1024 * 1024; // the gateway sends local files inline to fal, up to 20 MB each
  const POLL_MS = 2000;
  const ACCEPT = { image: 'image/*', video: 'video/*', audio: 'audio/*', file: 'image/*,video/*,audio/*' };
  // fal's named image sizes (fal schema docs): width x height, for the megapixel price.
  const SIZES = {
    square_hd: [1024, 1024], square: [512, 512], portrait_4_3: [768, 1024], portrait_16_9: [576, 1024],
    landscape_4_3: [1024, 768], landscape_16_9: [1024, 576],
  };

  const $ = (s, root = document) => root.querySelector(s);
  document.getElementById('pgStaleReload')?.addEventListener('click', () => location.reload());
  const EMPTY_CANVAS = document.getElementById('pgCanvasEmpty');
  const S = {
    test: false, keySet: false, offline: false,
    spec: null, values: {}, touched: new Set(), exampled: new Set(), tab: 'form',
    price: null, priceState: 'none', confirmed: new Set(),
    jobs: [], polls: new Map(), shown: null, shownOut: 0, tick: null,
    cats: [], cat: '', q: '', cursor: null, listSeq: 0, listHasMore: false,
    codeLang: (() => { try { const l = localStorage.getItem('pg.codeLang'); return ['curl', 'js', 'py'].includes(l) ? l : 'curl'; } catch (_) { return 'curl'; } })(), codeReq: null, pendingRun: null, loadSeq: 0, sending: false,
    templates: [], sampleIds: null, uploads: 0,
  };

  // ------------------------------------------------------------ helpers

  function el(tag, attrs, kids) {
    const n = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === 'class') n.className = v;
      else if (k === 'text') n.textContent = v;
      else if (k.startsWith('on') && typeof v === 'function') n.addEventListener(k.slice(2), v);
      else n.setAttribute(k, v === true ? '' : String(v));
    }
    for (const c of [].concat(kids || [])) {
      if (c === null || c === undefined || c === false) continue;
      n.append(c instanceof Node ? c : document.createTextNode(String(c)));
    }
    return n;
  }

  function icon(name) {
    const ns = 'http://www.w3.org/2000/svg';
    const svg = document.createElementNS(ns, 'svg');
    svg.setAttribute('class', 'ico');
    svg.setAttribute('aria-hidden', 'true');
    const use = document.createElementNS(ns, 'use');
    use.setAttribute('href', `#i-${name}`);
    svg.append(use);
    return svg;
  }

  const num = (v) => typeof v === 'number' && isFinite(v);

  function video(attrs) {
    const v = el('video', attrs);
    v.muted = true; // the property, so autoplay is allowed
    return v;
  }

  async function call(url, opts = {}) {
    const init = { method: opts.method || 'GET', headers: {} };
    if (opts.body !== undefined) {
      init.headers['content-type'] = 'application/json';
      init.body = JSON.stringify(opts.body);
    }
    try {
      const r = await fetch(url, init);
      let data = null;
      try { data = await r.json(); } catch (_) { data = null; }
      // The engine restarted: this page's launch cookie is old, and only a reload gets the new one.
      if (r.status === 403 && data && data.error === 'launch_token') { const b = $('#pgStale'); if (b) b.hidden = false; }
      return { ok: r.ok, status: r.status, data };
    } catch (_) {
      return { ok: false, status: 0, data: null };
    }
  }

  /** fal's unit as "per ___": megapixels -> megapixel, compute seconds -> compute second; 1000 characters stays. */
  function perUnit(unit) {
    const u = String(unit || '').trim();
    if (!u || /^\d/.test(u)) return u;
    return u.replace(/([a-z])s$/i, (m, c) => (c === 's' ? m : c));
  }

  function money(v) {
    if (!num(v) || v < 0) return '';
    if (v >= 0.1) return `$${v.toFixed(2)}`;
    if (v === 0) return '$0';
    if (v < 0.001) return `$${v.toLocaleString('en-US', { maximumSignificantDigits: 2, useGrouping: false })}`;
    const digits = v >= 0.01 ? 3 : 4;
    return `$${v.toFixed(digits).replace(/0+$/, '').replace(/\.$/, '')}`;
  }

  function ago(sec) {
    const d = Math.max(0, Date.now() / 1000 - sec);
    if (d < 60) return 'just now';
    if (d < 3600) return `${Math.floor(d / 60)} min ago`;
    if (d < 86400) return `${Math.floor(d / 3600)} h ago`;
    return new Date(sec * 1000).toLocaleDateString();
  }

  function clock(sec) {
    const s = Math.max(0, Math.floor(sec));
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  }

  const store = {
    get(k) { try { return localStorage.getItem(k); } catch (_) { return null; } },
    set(k, v) { try { localStorage.setItem(k, v); } catch (_) { /* private window */ } },
  };

  const ERRORS = {
    unknown_field: 'That setting is not part of this model.',
    invalid_value: 'That value is not accepted here.',
    field_required: 'This one is needed.',
    file_missing: 'A picked file is gone. Pick it again.',
    file_too_large: 'That file is over 20 MB. Use a smaller one, or paste a public link to it.',
    files_too_large: 'The files add up to more than 40 MB. Use smaller ones, or public links.',
    missing_key: 'Add your fal key above to run models.',
    too_many_jobs: 'Too many runs are going at once. Wait for a few to finish.',
    catalog_busy: 'fal is busy right now. Try again in a minute.',
    catalog_unavailable: 'Could not reach fal. Check your connection and try again.',
    unknown_fal_model: 'fal has no model with that id.',
    bad_model_id: 'That is not a fal model id.',
    not_runnable: 'This model cannot run in the Playground yet.',
    price_unavailable: 'fal did not give a price for this model.',
    bad_request: 'The request was not understood. Reload the page and try again.',
    launch_token: 'This page lost its link to the studio. Reload the page, then try again.',
  };
  const say = (code) => ERRORS[code] || 'Something went wrong. Try again.';

  /** Puts `nodes` in a live region only when the words differ, so a screen reader is not read the same line again. */
  function setLive(box, nodes) {
    const next = document.createElement('div');
    next.append(...nodes.filter(Boolean));
    if (next.innerHTML === box.innerHTML) return;
    box.replaceChildren(...next.childNodes);
  }

  /** One polite message for screen readers (the running clock is not announced). */
  function announce(msg) {
    const live = $('#pgLive');
    live.textContent = '';
    requestAnimationFrame(() => { live.textContent = msg; });
  }

  /** fal's field titles read as plain labels: "Image Url" -> "Image", "VoiceSetting" -> "Voice setting", "Num Images" ->
      "Number of images", "Cfg Scale" -> "CFG scale". */
  function prettyTitle(t) {
    let x = String(t || '').trim();
    if (/^[A-Z][a-z]+(?:[A-Z][a-z]+)+$/.test(x)) x = x.replace(/([a-z])([A-Z])/g, '$1 $2');
    x = x.replace(/\s+Urls$/i, 's').replace(/\s+Url$/i, '').replace(/^Num\s+/i, 'Number of ');
    x = x.split(/\s+/).map((w) => (/^[A-Z0-9]{2,}$/.test(w) && /[A-Z]/.test(w) ? w : w.toLowerCase())).join(' ');
    x = x.replace(/\b(cfg|fps|hd|lora|id|url|json|3d|sfx)\b/g, (m) => ({ lora: 'LoRA', '3d': '3D' }[m] || m.toUpperCase()));
    return x.charAt(0).toUpperCase() + x.slice(1);
  }

  /** The error for one field, with its allowed range when the problem is a number. */
  function fieldSay(name, code) {
    const f = S.spec && S.spec.fields.find((x) => x.name === name);
    if (code === 'invalid_value' && f && (f.kind === 'integer' || f.kind === 'number') && (num(f.min) || num(f.max))) return rangeText(f);
    return say(code);
  }

  function rangeText(f) {
    if (num(f.min) && num(f.max)) return `Use a number from ${f.min} to ${f.max}.`;
    if (num(f.min)) return `Use a number of ${f.min} or more.`;
    return `Use a number up to ${f.max}.`;
  }

  // ------------------------------------------------------------ status

  async function loadStatus() {
    const [h, k] = await Promise.all([call('/api/health'), call('/api/keys')]);
    S.offline = !h.ok;
    $('#pgOffline').hidden = !S.offline;
    S.test = !!(h.ok && h.data && h.data.fake_providers);
    $('#pgTest').hidden = !S.test;
    const fal = k.ok && k.data && (k.data.keys || []).find((x) => x.env === 'FAL_KEY');
    S.keySet = !!(fal && fal.set);
    $('#pgKey').hidden = S.offline || S.test || S.keySet;
    // A Compare opened before this answer may have asked for prices without a key.
    if (C.started && C.ids.some((id) => C.pstate.get(id) === 'nokey') && (S.test || S.keySet)) cmpRefreshPrices();
  }

  $('#pgKeyForm').addEventListener('submit', async (e) => {
    e.preventDefault();
    const input = $('#pgKeyInput');
    const value = input.value.trim();
    const note = $('#pgKeyNote');
    if (!value) { note.textContent = 'Paste the key first.'; return; }
    note.textContent = 'Saving…';
    const r = await call('/api/keys', { method: 'POST', body: { env: 'FAL_KEY', value } });
    input.value = '';
    if (!r.ok) { note.textContent = 'The key was not saved. Check it and try again.'; return; }
    note.textContent = '';
    S.keySet = true;
    $('#pgKey').hidden = true;
    loadPrice();
    cmpRefreshPrices();
  });

  // ------------------------------------------------------------ model and form

  // The gateway sends a missing default or example as null.
  const has = (v) => v !== undefined && v !== null;
  function fieldDefault(f) { return has(f.default) ? f.default : null; }

  function current(f) {
    return S.touched.has(f.name) ? S.values[f.name] : fieldDefault(f);
  }

  function setValue(name, v, opts = {}) {
    if (v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length)) {
      delete S.values[name];
      S.touched.delete(name);
    } else {
      S.values[name] = v;
      S.touched.add(name);
    }
    if (!opts.keepExample) S.exampled.delete(name);
    clearFieldError(name);
    renderPrice();
  }

  /** What Run sends: only what you set, and the examples a model needs to run. */
  function collect() {
    const out = {};
    if (!S.spec) return out;
    for (const f of S.spec.fields) {
      if (!S.touched.has(f.name)) continue;
      const v = S.values[f.name];
      if (v === undefined || v === null || v === '' || (Array.isArray(v) && !v.length)) continue;
      out[f.name] = v;
    }
    return out;
  }

  function initialValues(spec, preset) {
    S.values = {};
    S.touched = new Set();
    S.exampled = new Set();
    if (preset) {
      for (const [k, v] of Object.entries(preset)) {
        if (spec.fields.some((f) => f.name === k)) { S.values[k] = v; S.touched.add(k); }
      }
      return;
    }
    // A model opens ready to run: fal's own example goes into anything needed that has no default.
    for (const f of spec.fields) {
      const wanted = f.required || f.name === 'prompt' || f.name === 'text';
      if (wanted && !has(f.default) && has(f.example) && f.kind !== 'json') {
        S.values[f.name] = f.example;
        S.touched.add(f.name);
        S.exampled.add(f.name);
      }
    }
  }

  async function loadModel(id, preset) {
    const seq = ++S.loadSeq;
    if ($('#pgConfirm').open) $('#pgConfirm').close();
    S.pendingRun = null;
    if (!preset) $('#pgRunNote').textContent = '';
    $('#pgModelName').textContent = 'Loading…';
    $('#pgModelMeta').textContent = id;
    $('#pgRun').disabled = true;
    const r = await call(`/api/fal/model?id=${encodeURIComponent(id)}`);
    if (seq !== S.loadSeq) return;
    if (!r.ok || !r.data || !r.data.model) {
      $('#pgModelName').textContent = 'Could not load this model';
      $('#pgModelMeta').textContent = id;
      $('#pgModelDesc').textContent = say(r.data && r.data.error) + (r.status === 404 ? ' Pick another one.' : '');
      $('#pgFields').textContent = '';
      $('#pgMore').hidden = true;
      if (r.status === 404 && id !== DEFAULT_MODEL) store.set('pg.model', DEFAULT_MODEL);
      return;
    }
    const spec = r.data.model;
    S.spec = spec;
    S.price = null;
    S.priceState = 'none';
    initialValues(spec, preset);
    store.set('pg.model', spec.id);
    if (S.mode !== 'compare') history.replaceState(null, '', `#m=${encodeURIComponent(spec.id)}`);
    renderModelHead();
    renderForm();
    if (S.tab === 'json') showJson();
    renderPrice();
    loadPrice();
  }

  function catIcon(cat) {
    const c = cat || '';
    if (c.endsWith('-3d')) return '3d';
    if (c.endsWith('-video')) return 'video';
    if (c.endsWith('-audio') || c.endsWith('-speech')) return 'audio';
    return 'image';
  }

  function catLabel(id) {
    const c = S.cats.find((x) => x.id === id);
    return c ? c.label : (id || '').replace(/-/g, ' ');
  }

  function renderModelHead() {
    const s = S.spec;
    $('#pgModelName').textContent = s.name;
    $('#pgModelMeta').textContent = `${catLabel(s.category)} · ${s.id}`;
    const thumb = $('#pgModelThumb');
    thumb.textContent = '';
    if (s.thumb) thumb.append(el('img', { src: s.thumb, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' }));
    else thumb.append(icon(catIcon(s.category)));
    const desc = $('#pgModelDesc');
    desc.textContent = s.description || '';
    desc.classList.remove('open');
    const links = $('#pgModelLinks');
    links.textContent = '';
    const more = el('button', { type: 'button', class: 'pg-link-btn', 'aria-expanded': 'false', hidden: true }, ['More']);
    more.addEventListener('click', () => {
      const open = desc.classList.toggle('open');
      more.textContent = open ? 'Less' : 'More';
      more.setAttribute('aria-expanded', String(open));
    });
    links.append(more);
    if (s.page) links.append(el('a', { href: s.page, target: '_blank', rel: 'noopener', class: 'pg-out' }, ['fal page ', el('span', { 'aria-hidden': 'true' }, ['↗'])]));
    // Only offer More when the text is really cut off.
    requestAnimationFrame(() => { more.hidden = desc.scrollHeight <= desc.clientHeight + 2; });
    const why = $('#pgWhyNot');
    const text = s.runnable ? (s.note || '') : (s.why_not || say('not_runnable'));
    why.hidden = !text;
    why.textContent = text;
  }

  function renderForm() {
    const main = $('#pgFields');
    const more = $('#pgMoreFields');
    main.textContent = '';
    more.textContent = '';
    const adv = S.spec.fields.filter((f) => f.advanced);
    for (const f of S.spec.fields) (f.advanced ? more : main).append(fieldRow(f));
    $('#pgMore').hidden = !adv.length;
    $('#pgMoreLabel').textContent = `More settings (${adv.length})`;
    if (!main.children.length) main.append(el('p', { class: 'pg-note' }, ['This model has no main settings. Open More settings, or press Run.']));
    $('#pgRun').disabled = !S.spec.runnable || S.offline || S.sending;
  }

  function helpText(f) {
    const parts = [];
    let d = String(f.description || '').trim();
    // Under an upload box "URL of the image to be used for the video." reads as if a link were needed.
    if (f.kind === 'media' || f.kind === 'medialist') {
      d = d.replace(/^(?:the\s+)?urls?\s+(?:of|to|for)\s+(?:(?:the|an?)\s+(?=\S)|(?=(?:image|video|audio|input|file|mask|photo)s?\b))/i, 'The ');
    }
    if (d) parts.push(/[.!?]$/.test(d) ? d : `${d}.`);
    if (has(f.default) && f.kind !== 'longtext' && f.kind !== 'media') {
      const shown = typeof f.default === 'object' ? JSON.stringify(f.default) : f.kind === 'choice' ? choiceLabel(f.default, f.choices) : String(f.default);
      parts.push(`Default: ${shown}.`);
    }
    if (f.limits) parts.push(f.limits);
    return parts.join(' ');
  }

  function fieldRow(f) {
    const id = `f_${f.name}`;
    const row = el('div', { class: `pg-field kind-${f.kind}`, 'data-name': f.name });
    const head = el('div', { class: 'pg-field-head' }, [
      el('label', { for: id, id: `${id}_l` }, [prettyTitle(f.title || f.name)]),
      f.required ? el('span', { class: 'pg-tag', title: 'The model cannot run without it.' }, ['needed']) : null,
      S.exampled.has(f.name) ? el('span', { class: 'pg-tag ex', title: "fal's example. Replace it with your own." }, ['example']) : null,
    ]);
    row.append(head, control(f, id));
    const help = helpText(f);
    // fal writes syntax in backticks: show it as code, not with the backticks.
    if (help) row.append(el('p', { class: 'pg-help', id: `${id}_h` }, help.split(/`([^`]+)`/).map((t, i) => (i % 2 ? el('code', {}, [t]) : t))));
    row.append(el('p', { class: 'pg-err', id: `${id}_e`, role: 'alert' }));
    return row;
  }

  function markTouched(row) {
    const ex = row && row.querySelector('.pg-tag.ex');
    if (ex) ex.remove();
  }

  function control(f, id) {
    switch (f.kind) {
      case 'longtext': case 'text': return textControl(f, id);
      case 'integer': case 'number': return numberControl(f, id);
      case 'boolean': return switchControl(f, id);
      case 'choice': return choiceControl(f, id);
      case 'media': return mediaControl(f, id, false);
      case 'medialist': return mediaControl(f, id, true);
      default: return jsonControl(f, id);
    }
  }

  function autosize(t) {
    t.style.height = 'auto';
    t.style.height = `${Math.min(t.scrollHeight + 2, 420)}px`;
  }

  /** fal's enum beside a free text or number field, offered as suggestions (any other value still goes). */
  function suggestions(f, id) {
    const list = (f.choices || []).filter((c) => typeof c === 'string' || typeof c === 'number');
    return list.length ? el('datalist', { id: `${id}_list` }, list.map((c) => el('option', { value: String(c) }))) : null;
  }

  /** The control plus its suggestion list, when it has one. */
  function withList(n, list) {
    if (!list) return n;
    const frag = document.createDocumentFragment();
    frag.append(n, list);
    return frag;
  }

  function textControl(f, id) {
    const list = suggestions(f, id);
    const long = f.kind === 'longtext' && !list;
    const v = S.touched.has(f.name) ? S.values[f.name] : '';
    const attrs = { id, 'aria-describedby': `${id}_h ${id}_e`, maxlength: f.max_len || null, spellcheck: long ? 'true' : 'false' };
    if (!S.touched.has(f.name) && has(f.default) && f.default !== '') attrs.placeholder = String(f.default);
    const n = long
      ? el('textarea', { ...attrs, rows: f.name === 'prompt' || f.name === 'text' ? 4 : 2 })
      : el('input', { ...attrs, type: 'text', list: list ? list.id : null, autocomplete: list ? 'off' : null });
    n.value = typeof v === 'string' ? v : '';
    n.addEventListener('input', () => {
      setValue(f.name, n.value);
      markTouched(n.closest('.pg-field'));
      if (long) autosize(n);
    });
    if (long) requestAnimationFrame(() => autosize(n));
    return withList(n, list);
  }

  function numberStep(f) {
    if (f.kind === 'integer') return 1;
    const span = (f.max ?? 0) - (f.min ?? 0);
    if (f.min !== null && f.max !== null && span <= 1) return 0.01;
    if (f.min !== null && f.max !== null && span <= 20) return 0.1;
    return 'any';
  }

  function numberControl(f, id) {
    const wrap = el('div', { class: 'pg-num' });
    const step = numberStep(f);
    const list = suggestions(f, id);
    // A word among fal's choices ("auto") cannot go in a number box, so such a field takes text.
    const words = (f.choices || []).filter((c) => typeof c === 'string');
    const box = words.length
      ? el('input', { id, type: 'text', inputmode: 'decimal', list: list.id, autocomplete: 'off', 'aria-describedby': `${id}_h ${id}_e` })
      : el('input', { id, type: 'number', inputmode: f.kind === 'integer' ? 'numeric' : 'decimal', step, min: f.min, max: f.max, list: list ? list.id : null, 'aria-describedby': `${id}_h ${id}_e` });
    const v = current(f);
    if (S.touched.has(f.name)) box.value = String(S.values[f.name]);
    else if (v !== null) box.placeholder = String(v);
    else if (f.name === 'seed') box.placeholder = 'random';
    const slider = f.min !== null && f.max !== null && f.max - f.min <= 100000
      ? el('input', { type: 'range', min: f.min, max: f.max, step: step === 'any' ? (f.max - f.min) / 100 : step, 'aria-labelledby': `${id}_l`, tabindex: '-1' })
      : null;
    if (slider) slider.value = String(num(v) ? v : f.min);
    const put = (raw, from) => {
      if (raw === '' || raw === null) { setValue(f.name, null); if (slider) slider.value = String(num(fieldDefault(f)) ? fieldDefault(f) : f.min); return; }
      let n = Number(raw);
      if (!isFinite(n)) return;
      if (f.kind === 'integer') n = Math.round(n);
      setValue(f.name, n);
      markTouched(wrap.closest('.pg-field'));
      if (from !== box) box.value = String(n);
      if (slider && from !== slider) slider.value = String(n);
    };
    box.addEventListener('input', () => {
      const raw = box.value.trim();
      if (words.includes(raw)) { setValue(f.name, raw); markTouched(wrap.closest('.pg-field')); return; }
      // A number still being typed ("-", ".", "1e"): nothing is set until it is one.
      if (raw !== '' && /^[-+]?\.?$|^[-+]?\d*\.?\d*[eE][-+]?$/.test(raw)) { put('', box); return; }
      const n = Number(raw);
      if (raw !== '' && !isFinite(n)) { showFieldError(f.name, words.length ? `Use a number or ${words.map((w) => `"${w}"`).join(' or ')}.` : 'Use a number.'); return; }
      const out = raw !== '' && ((num(f.min) && (f.min_strict ? n <= f.min : n < f.min)) || (num(f.max) && (f.max_strict ? n >= f.max : n > f.max)));
      if (out) { showFieldError(f.name, rangeText(f)); return; }
      put(raw, box);
    });
    if (slider) {
      slider.addEventListener('input', () => put(slider.value, slider));
      wrap.append(slider);
    }
    wrap.append(box);
    if (list) wrap.append(list);
    return wrap;
  }

  function switchControl(f, id) {
    const on = !!current(f);
    const b = el('button', { type: 'button', id, role: 'switch', class: 'pg-switch', 'aria-checked': String(on), 'aria-describedby': `${id}_h ${id}_e` }, [el('span', { class: 'pg-knob', 'aria-hidden': 'true' })]);
    b.addEventListener('click', () => {
      const next = b.getAttribute('aria-checked') !== 'true';
      b.setAttribute('aria-checked', String(next));
      // Back to the model's own default means "not sent".
      setValue(f.name, has(f.default) && next === !!f.default ? null : next);
    });
    return b;
  }

  /** landscape_16_9 -> Landscape 16:9, square_hd -> Square HD; anything else as fal wrote it, except a plain word
      among such choices ("square" beside "square_hd"), which is capitalised to match. */
  function choiceLabel(c, all) {
    if (typeof c !== 'string') return String(c);
    if (/^[a-z]+$/.test(c) && Array.isArray(all) && all.some((x) => typeof x === 'string' && /^[a-z0-9]+(_[a-z0-9]+)+$/.test(x))) {
      return c.charAt(0).toUpperCase() + c.slice(1);
    }
    if (!/^[a-z0-9_]+$/.test(c) || !c.includes('_')) return c;
    const t = c.replace(/_(\d+)_(\d+)$/, ' $1:$2').replace(/_/g, ' ').replace(/\bhd\b/g, 'HD');
    return t.charAt(0).toUpperCase() + t.slice(1);
  }

  function sameValue(a, b) {
    if (typeof a === 'number' || typeof b === 'number') return Number(a) === Number(b);
    return a === b;
  }

  function choiceControl(f, id) {
    const cur = current(f);
    const isCustom = cur !== null && typeof cur === 'object';
    const short = f.choices.length <= 6 && f.choices.every((c) => choiceLabel(c, f.choices).length <= 14) && !f.custom_json;
    if (short) {
      const g = el('div', { class: 'pg-choices', role: 'radiogroup', id, 'aria-labelledby': `${id}_l`, 'aria-describedby': `${id}_h ${id}_e` });
      f.choices.forEach((c, i) => {
        const on = cur !== null && sameValue(c, cur);
        const label = f.name === 'duration' && /^\d+(\.\d+)?$/.test(String(c)) ? `${c} s` : choiceLabel(c, f.choices);
        const b = el('button', { type: 'button', role: 'radio', 'aria-checked': String(on), tabindex: on || (cur === null && i === 0) ? '0' : '-1' }, [label]);
        b.addEventListener('click', () => {
          g.querySelectorAll('[role=radio]').forEach((x) => { x.setAttribute('aria-checked', 'false'); x.tabIndex = -1; });
          b.setAttribute('aria-checked', 'true');
          b.tabIndex = 0;
          setValue(f.name, c); // an explicit pick is always sent
        });
        b.addEventListener('keydown', (e) => {
          const all = [...g.querySelectorAll('[role=radio]')];
          const at = all.indexOf(b);
          const d = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
          if (!d) return;
          e.preventDefault();
          const next = all[(at + d + all.length) % all.length];
          next.focus();
          next.click();
        });
        g.append(b);
      });
      return g;
    }
    const wrap = el('div', { class: 'pg-select-wrap' });
    const sel = el('select', { id, 'aria-describedby': `${id}_h ${id}_e` });
    if (cur === null) sel.append(el('option', { value: '' }, ['Model default']));
    f.choices.forEach((c, i) => sel.append(el('option', { value: String(i) }, [choiceLabel(c, f.choices)])));
    if (f.custom_json) sel.append(el('option', { value: 'custom' }, ['Custom…']));
    const custom = f.custom_json ? el('textarea', { class: 'pg-custom', rows: 2, spellcheck: 'false', 'aria-label': `${f.title} as JSON`, placeholder: '{"width": 1024, "height": 1024}' }) : null;
    if (isCustom) {
      sel.value = 'custom';
      custom.value = JSON.stringify(cur);
    } else if (cur !== null) {
      const i = f.choices.findIndex((c) => sameValue(c, cur));
      sel.value = i >= 0 ? String(i) : '';
    }
    if (custom) custom.hidden = sel.value !== 'custom';
    sel.addEventListener('change', () => {
      if (custom) custom.hidden = sel.value !== 'custom';
      if (sel.value === '') { setValue(f.name, null); return; }
      if (sel.value === 'custom') { custom.focus(); return; }
      setValue(f.name, f.choices[Number(sel.value)]);
    });
    if (custom) {
      custom.addEventListener('input', () => {
        try {
          const v = JSON.parse(custom.value);
          if (v && typeof v === 'object' && !Array.isArray(v)) { setValue(f.name, v); showFieldError(f.name, ''); }
          else showFieldError(f.name, 'Write a JSON object, like {"width": 1024, "height": 1024}.');
        } catch (_) { showFieldError(f.name, 'That is not valid JSON yet.'); }
      });
    }
    wrap.append(sel);
    if (custom) wrap.append(custom);
    return wrap;
  }

  function jsonControl(f, id) {
    const t = el('textarea', { id, rows: 3, spellcheck: 'false', class: 'pg-mono', 'aria-describedby': `${id}_h ${id}_e`, placeholder: f.example !== undefined && f.example !== null ? JSON.stringify(f.example) : 'JSON' });
    if (S.touched.has(f.name)) t.value = JSON.stringify(S.values[f.name], null, 1);
    t.addEventListener('input', () => {
      if (!t.value.trim()) { setValue(f.name, null); return; }
      try { setValue(f.name, JSON.parse(t.value)); } catch (_) { showFieldError(f.name, 'That is not valid JSON yet.'); }
    });
    return t;
  }

  function kindOfUrl(u, fallback) {
    const ext = (u.split(/[?#]/)[0].split('.').pop() || '').toLowerCase();
    if (['png', 'jpg', 'jpeg', 'webp', 'gif', 'avif'].includes(ext)) return 'image';
    if (['mp4', 'mov', 'webm', 'm4v'].includes(ext)) return 'video';
    if (['mp3', 'wav', 'm4a', 'ogg', 'flac', 'aac'].includes(ext)) return 'audio';
    return fallback;
  }

  function preview(url, kind) {
    const k = kindOfUrl(url, kind);
    if (k === 'video') return video({ src: url, playsinline: true, preload: 'metadata', loop: true });
    if (k === 'audio') return el('audio', { src: url, controls: true, preload: 'none' });
    if (k === 'image') {
      const img = el('img', { src: url, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' });
      // The run still sends the link; only this browser could not show it.
      img.addEventListener('error', () => img.replaceWith(el('span', { class: 'pg-file' }, ['No preview'])), { once: true });
      return img;
    }
    return el('span', { class: 'pg-file' }, [url.split('/').pop().slice(0, 40)]);
  }

  async function uploadFile(file, kind) {
    if (file.size > MAX_FILE) return { error: 'That file is over 20 MB. Use a smaller one, or paste a public link to it.' };
    const top = (file.type || '').split('/')[0];
    if (kind !== 'file' && top !== kind) return { error: `This takes ${kind === 'image' || kind === 'audio' ? 'an' : 'a'} ${kind} file.` };
    const data = await new Promise((resolve) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result);
      r.onerror = () => resolve(null);
      r.readAsDataURL(file);
    });
    if (!data) return { error: 'The file could not be read.' };
    const r = await call('/api/uploads', { method: 'POST', body: { data } });
    if (!r.ok) return { error: r.data && r.data.error === 'unsupported_file' ? 'That file type is not supported.' : 'The upload failed. Try again.' };
    return { url: r.data.url };
  }

  function mediaControl(f, id, many) {
    return mediaPicker({
      id, many, kind: f.media || 'file', label: f.title, max: many ? Math.min(f.max_len || 16, 16) : 1,
      get: () => (S.touched.has(f.name) ? S.values[f.name] : null),
      set: (v) => setValue(f.name, v),
      error: (msg) => showFieldError(f.name, msg),
    });
  }

  /** Adds every link typed into a file field but not added yet (Run must not go without it). True when one was turned
      down: an added link leaves its box empty. */
  function commitLinks(root) {
    if (!root) return false;
    let bad = false;
    root.querySelectorAll('.pg-link').forEach((l) => {
      if (!l.value.trim()) return;
      l.dispatchEvent(new Event('change'));
      if (l.isConnected && l.value.trim()) bad = true;
    });
    return bad;
  }

  /** Files for one value: upload, drop or paste a link. `get`/`set` hold the value (a URL, or a list of them); `busy`
      hears when an upload starts and ends (S.uploads counts them, and no Run goes while one is on its way). */
  function mediaPicker({ id, many, kind, label, max, get, set, error, busy }) {
    const wrap = el('div', { class: 'pg-media', id, tabindex: '-1', 'aria-describedby': `${id}_h ${id}_e` });
    const touched = () => markTouched(wrap.closest('.pg-field'));
    const list = () => {
      const v = get();
      if (many) return Array.isArray(v) ? v.slice() : [];
      return v ? [v] : [];
    };
    const save = (items) => set(many ? items : (items[0] || null));
    const draw = () => {
      wrap.textContent = '';
      const items = list();
      const tiles = el('div', { class: 'pg-tiles' });
      items.forEach((u, i) => {
        const rm = el('button', { type: 'button', class: 'pg-tile-x', 'aria-label': 'Remove this file' }, [icon('close')]);
        rm.addEventListener('click', () => { const next = list(); next.splice(i, 1); save(next); touched(); draw(); });
        tiles.append(el('div', { class: 'pg-tile' }, [preview(u, kind), rm]));
      });
      if (items.length < max) {
        const file = el('input', { type: 'file', accept: ACCEPT[kind] || ACCEPT.file, multiple: many, class: 'sr', tabindex: '-1', 'aria-hidden': 'true' });
        const drop = el('button', { type: 'button', class: 'pg-drop' }, [
          icon('upload'),
          el('span', {}, [items.length ? 'Add another' : `Upload ${kind === 'file' ? 'a file' : `${kind === 'image' || kind === 'audio' ? 'an' : 'a'} ${kind === 'audio' ? 'audio file' : kind}`}`]),
          el('small', {}, ['or drop it here']),
        ]);
        const take = async (files) => {
          const next = list();
          for (const fl of [...files].slice(0, max - next.length)) {
            drop.disabled = true;
            drop.querySelector('span').textContent = 'Uploading…';
            S.uploads += 1;
            if (busy) busy();
            const r = await uploadFile(fl, kind).finally(() => { S.uploads -= 1; if (busy) busy(); if (C.started) renderCmpPrice(); });
            // Another model opened meanwhile: this field is gone, and the file is not put into the new one.
            if (!wrap.isConnected) return;
            drop.disabled = false;
            if (r.error) { error(r.error); draw(); return; }
            next.push(r.url);
          }
          save(next);
          touched();
          draw();
        };
        drop.addEventListener('click', () => file.click());
        file.addEventListener('change', () => { if (file.files.length) take(file.files); });
        drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
        drop.addEventListener('dragleave', () => drop.classList.remove('over'));
        drop.addEventListener('drop', (e) => { e.preventDefault(); drop.classList.remove('over'); if (e.dataTransfer.files.length) take(e.dataTransfer.files); });
        tiles.append(drop, file);
        const link = el('input', { type: 'url', class: 'pg-link', placeholder: 'or paste a link: https://…', 'aria-label': `${label}: paste a link`, spellcheck: 'false' });
        const addLink = () => {
          const u = link.value.trim();
          if (!u) return;
          if (!/^https:\/\/[^\s]+$/i.test(u)) { error('Links must start with https://.'); return; }
          const next = list();
          next.push(u);
          link.value = '';
          save(next);
          touched();
          draw();
        };
        link.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); addLink(); } });
        link.addEventListener('change', addLink);
        wrap.append(tiles, link);
      } else {
        wrap.append(tiles);
      }
    };
    draw();
    return wrap;
  }

  function showFieldError(name, msg) {
    const row = document.querySelector(`.pg-field[data-name="${CSS.escape(name)}"]`);
    if (!row) return;
    const e = row.querySelector('.pg-err');
    e.textContent = msg || '';
    row.classList.toggle('bad', !!msg);
    const ctl = document.getElementById(`f_${name}`);
    if (ctl) { if (msg) ctl.setAttribute('aria-invalid', 'true'); else ctl.removeAttribute('aria-invalid'); }
    if (msg && row.closest('details')) row.closest('details').open = true;
  }

  function clearFieldError(name) { showFieldError(name, ''); }

  // ------------------------------------------------------------ Form / JSON

  function showJson() {
    $('#pgJson').value = JSON.stringify(collect(), null, 2);
    $('#pgJsonNote').textContent = 'This is exactly what Run sends. Fields you leave out use the model\'s defaults.';
  }

  /** Applies the JSON box to the form. False (and a note) when it does not parse. */
  function applyJson() {
    let v;
    try { v = JSON.parse($('#pgJson').value || '{}'); } catch (_) {
      $('#pgJsonNote').textContent = 'That is not valid JSON yet. Fix it, or press Reset.';
      return false;
    }
    if (!v || typeof v !== 'object' || Array.isArray(v)) {
      $('#pgJsonNote').textContent = 'Write one JSON object: {"prompt": "…"}.';
      return false;
    }
    const unknown = Object.keys(v).filter((k) => !S.spec.fields.some((f) => f.name === k));
    if (unknown.length) {
      $('#pgJsonNote').textContent = `Not part of this model: ${unknown.join(', ')}. Remove ${unknown.length === 1 ? 'it' : 'them'} first.`;
      return false;
    }
    S.values = {};
    S.touched = new Set();
    S.exampled = new Set();
    for (const [k, val] of Object.entries(v)) {
      if (val === null || val === undefined) continue;
      S.values[k] = val;
      S.touched.add(k);
    }
    return true;
  }

  /** `discard` leaves the JSON box behind without applying it (a template replaces everything anyway). */
  function setTab(tab, discard = false) {
    if (tab === S.tab || !S.spec) return;
    if (tab === 'json' && S.uploads > 0) { $('#pgRunNote').textContent = 'A file is still uploading. Switch to JSON when it shows.'; return; }
    if (tab === 'form' && !discard && !applyJson()) return;
    S.tab = tab;
    $('#pgTabForm').setAttribute('aria-selected', String(tab === 'form'));
    $('#pgTabJson').setAttribute('aria-selected', String(tab === 'json'));
    $('#pgForm').hidden = tab !== 'form';
    $('#pgJsonWrap').hidden = tab !== 'json';
    if (tab === 'json') { showJson(); $('#pgJson').focus(); } else { renderForm(); renderPrice(); }
  }

  // Enter in a lone text box would submit the form and reload the page.
  $('#pgForm').addEventListener('submit', (e) => e.preventDefault());
  /** Left and right arrows move between the tabs of a tab list, as screen-reader users expect. */
  function arrowTabs(list) {
    list.addEventListener('keydown', (e) => {
      const tabs = [...list.querySelectorAll('[role=tab]')];
      const at = tabs.indexOf(document.activeElement);
      const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
      const end = e.key === 'Home' ? 0 : e.key === 'End' ? tabs.length - 1 : -1;
      if (at < 0 || (!d && end < 0)) return;
      e.preventDefault();
      const next = end >= 0 ? tabs[end] : tabs[(at + d + tabs.length) % tabs.length];
      next.focus();
      next.click();
      const sel = tabs.find((t) => t.getAttribute('aria-selected') === 'true');
      if (sel && sel !== next) sel.focus();
    });
  }
  arrowTabs($('.pg-toolbar .pg-seg'));
  arrowTabs($('#pgCodeTabs'));
  $('#pgTabForm').addEventListener('click', () => setTab('form'));
  $('#pgTabJson').addEventListener('click', () => setTab('json'));
  $('#pgJson').addEventListener('input', () => {
    try { JSON.parse($('#pgJson').value || '{}'); $('#pgJsonNote').textContent = 'This is exactly what Run sends. Fields you leave out use the model\'s defaults.'; } catch (_) { $('#pgJsonNote').textContent = 'Not valid JSON yet.'; }
    renderPrice();
  });
  $('#pgReset').addEventListener('click', () => {
    if (!S.spec) return;
    initialValues(S.spec, null);
    renderForm();
    if (S.tab === 'json') showJson();
    renderPrice();
    $('#pgRunNote').textContent = 'Back to the model\'s example and defaults.';
  });

  // ------------------------------------------------------------ price

  async function loadPrice() {
    if (!S.spec) return;
    const id = S.spec.id;
    if (!S.test && !S.keySet) { S.priceState = 'nokey'; renderPrice(); return; }
    S.priceState = 'loading';
    renderPrice();
    const r = await call(`/api/fal/price?id=${encodeURIComponent(id)}`);
    if (!S.spec || S.spec.id !== id) return;
    if (r.ok && r.data && r.data.price) { S.price = r.data.price; S.priceState = 'ok'; } else { S.price = null; S.priceState = 'none'; }
    const note = $('#pgRunNote');
    if (note.textContent.startsWith("fal's price is still loading")) note.textContent = '';
    renderPrice();
  }

  function fieldValue(name) {
    const f = S.spec && S.spec.fields.find((x) => x.name === name);
    if (!f) return undefined;
    if (S.tab === 'json') {
      try { const v = JSON.parse($('#pgJson').value || '{}'); if (name in v) return v[name]; } catch (_) { /* keep form value */ }
    }
    return current(f);
  }

  // Options that can change fal's rate for a model (fal's API gives one unit price): with any of them, totals say "about".
  const RATE_OPTIONS = ['generate_audio', 'audio', 'enable_audio', 'resolution', 'quality', 'video_quality', 'fps', 'mode'];

  /** Units times fal's list price, where the settings say how many units a run is. */
  function estimate() {
    return S.spec ? estimateFor(S.spec, S.price, fieldValue) : null;
  }

  /** The same for any model: `p` is its price, `fieldValue(name)` what a run would use for that field. */
  function estimateFor(spec, p, fieldValue) {
    if (!p || !num(p.unit_price)) return null;
    const unit = String(p.unit || '').toLowerCase().trim();
    const u = unit.replace(/s$/, '');
    const count = Number(fieldValue('num_images')) || 1;
    const sure = !spec.fields.some((f) => RATE_OPTIONS.includes(f.name));
    if (u === 'image') return { total: p.unit_price * count, exact: sure, how: `${count} image${count === 1 ? '' : 's'}` };
    if (u === 'second') {
      const d = parseFloat(String(fieldValue('duration') ?? '').replace(/s$/, ''));
      if (isFinite(d) && d > 0) return { total: p.unit_price * d, exact: sure, how: `${d} s` };
      return null;
    }
    if (['video', 'request', 'generation', 'call', 'run', 'output', 'song', 'model', '3d model'].includes(u)) {
      return { total: p.unit_price, exact: sure, how: `1 ${u}` };
    }
    if (u === 'megapixel') {
      const size = fieldValue('image_size');
      const wh = typeof size === 'string' ? SIZES[size] : (size && size.width && size.height ? [size.width, size.height] : null);
      if (!wh) return null;
      const mp = (wh[0] * wh[1]) / 1e6;
      return { total: p.unit_price * mp * count, exact: false, how: `${count} image${count === 1 ? '' : 's'} at ${mp.toFixed(1)} megapixels` };
    }
    if (u === '1000 character') {
      const text = String(fieldValue('prompt') ?? fieldValue('text') ?? '');
      const k = Math.max(1, Math.ceil(text.length / 1000));
      // fal states a price per 1000 characters and nothing about a shorter text, so this is the most it can be (when
      // no option can change the rate).
      return { total: p.unit_price * k, exact: false, upTo: sure, how: `${text.length} characters` };
    }
    return null;
  }

  /** The word before an estimate: none when exact, "up to" for a ceiling, else "about". */
  const approx = (est) => (est.exact ? '' : est.upTo ? 'up to ' : 'about ');

  /** What a run sends for Agave's spend log: the estimate shown before Run, or nothing when there was none. */
  const spendNote = (est) => (est && num(est.total) ? { usd: est.total, kind: est.exact ? 'exact' : est.upTo ? 'up to' : 'about', how: est.how || '' } : undefined);

  /** True while a run of the model on screen is still going (the button then says Run another). */
  const runningHere = () => !!S.spec && S.jobs.some((j) => isActive(j) && j.model === `fal:${S.spec.id}`);

  function priceSummary() {
    if (S.test) return { label: `${runningHere() ? 'Run another' : 'Run'} · test, free`, exact: true, est: estimate() };
    const est = estimate();
    const avg = S.price && S.price.average;
    if (est && est.exact) return { label: `Run · ${money(est.total)}`, exact: true, est, avg };
    if (est) return { label: `Run · ${approx(est)}${money(est.total)}`, exact: false, est, avg };
    if (num(avg)) return { label: `Run · about ${money(avg)}`, exact: false, avg };
    return { label: 'Run · check the price', exact: false };
  }

  function renderPrice() {
    const live = $('#pgPrice');
    const box = document.createElement('div');
    const done = () => setLive(live, [...box.childNodes]);
    if (!S.spec) { done(); return; }
    const sum = priceSummary();
    const again = runningHere() && !S.test ? sum.label.replace(/^Run/, 'Run another') : sum.label;
    if (!S.sending) $('#pgRunLabel').textContent = S.spec.runnable ? again : 'Not runnable here';
    const p = S.price;
    if (S.test) {
      box.append(el('span', {}, ['Test mode: runs are free and fake.']));
      if (p && num(p.unit_price)) {
        const real = sum.est ? ` A real run: ${sum.est.how} → ${approx(sum.est)}${money(sum.est.total)}.` : '';
        box.append(el('span', { class: 'muted' }, [` fal lists ${money(p.unit_price)} per ${perUnit(p.unit)}.${real}`]));
      }
      done();
      return;
    }
    if (S.priceState === 'nokey') { box.append(el('span', { class: 'muted' }, ['Add your fal key to see the price.'])); done(); return; }
    if (S.priceState === 'loading') { box.append(el('span', { class: 'muted' }, ['Asking fal for the price…'])); done(); return; }
    if (!p) {
      box.append(el('span', { class: 'warn' }, ['fal did not give a price for this model. ']), el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['Check it on fal ↗']));
      done();
      return;
    }
    const line = el('p', { class: 'pg-price-line' });
    if (num(p.unit_price)) line.append(el('b', {}, [money(p.unit_price)]), ` per ${perUnit(p.unit)}`);
    if (sum.est) line.append(el('span', { class: 'muted' }, [` · ${sum.est.how} → ${approx(sum.est)}${money(sum.est.total)}`]));
    if (num(p.average)) line.append(el('span', { class: 'muted' }, [` · fal's average run ${money(p.average)}`]));
    box.append(line, el('p', { class: 'pg-price-note' }, ['fal\'s list price, on your key. Options such as audio, resolution or quality can change the rate: ', el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['full table on fal ↗'])]));
    done();
  }

  // ------------------------------------------------------------ run

  function confirmText(sum) {
    const p = S.price;
    const parts = [];
    if (p && num(p.unit_price)) parts.push(el('p', {}, ['fal lists ', el('b', {}, [money(p.unit_price)]), ` per ${perUnit(p.unit)} for ${S.spec.name}.`]));
    if (sum.est) parts.push(el('p', {}, [`For these settings (${sum.est.how}) that is ${approx(sum.est)}`, el('b', {}, [money(sum.est.total)]), '.']));
    if (p && num(p.average)) parts.push(el('p', {}, ['fal\'s average for one run of this model is ', el('b', {}, [money(p.average)]), '.']));
    if (!p) parts.push(el('p', {}, ['fal did not give a price for this model, so the studio cannot show one.']));
    parts.push(el('p', { class: 'muted' }, ['Options such as audio, resolution or quality can change fal\'s rate. ', el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['See the full table on fal ↗']), ' It is paid to fal on your key. You are asked once per model.']));
    return parts;
  }

  async function run() {
    // Every way to Run (the button, Cmd+Enter, a form re-rendered mid-send) stops here while a run is being sent.
    if (!S.spec || !S.spec.runnable || S.sending) return;
    if (S.uploads > 0) { $('#pgRunNote').textContent = 'A file is still uploading. Press Run when it shows.'; return; }
    if (S.tab === 'form') commitLinks($('#pgForm'));
    if (matchMedia('(pointer: coarse)').matches && document.activeElement && document.activeElement.blur) document.activeElement.blur();
    if (S.tab === 'json' && !applyJson()) return;
    const input = collect();
    const wrong = S.tab === 'form' && document.querySelector('#pgForm .pg-field.bad');
    if (wrong) {
      const ctl = document.getElementById(`f_${wrong.dataset.name}`);
      if (ctl) ctl.focus();
      $('#pgRunNote').textContent = 'Fix the marked setting first.';
      return;
    }
    const missing = S.spec.fields.filter((f) => f.required && !(f.name in input));
    if (missing.length) {
      setTab('form');
      missing.forEach((f) => showFieldError(f.name, ERRORS.field_required));
      const first = document.getElementById(`f_${missing[0].name}`);
      if (first) first.focus();
      $('#pgRunNote').textContent = `Fill in ${missing.map((f) => prettyTitle(f.title)).join(', ')} first.`;
      return;
    }
    if (!S.test && !S.keySet) {
      $('#pgKey').hidden = false;
      $('#pgKeyInput').focus();
      $('#pgRunNote').textContent = ERRORS.missing_key;
      return;
    }
    if (!S.test && !S.confirmed.has(S.spec.id)) {
      // The price question is asked once per model, so never before fal's price has had its chance to arrive.
      if (S.priceState === 'loading') {
        $('#pgRunNote').textContent = "fal's price is still loading. Press Run again in a moment.";
        return;
      }
      $('#pgConfirmTitle').textContent = 'Check the price first';
      $('#pgConfirmRunLabel').textContent = 'Run it';
      const box = $('#pgConfirmText');
      box.textContent = '';
      box.append(...confirmText(priceSummary()));
      S.pendingRun = { id: S.spec.id, input };
      $('#pgConfirm').showModal();
      return;
    }
    await send(S.spec.id, input);
  }

  $('#pgConfirmRun').addEventListener('click', async () => {
    const pending = S.pendingRun;
    S.pendingRun = null;
    $('#pgConfirm').close();
    if (pending && pending.compare) {
      // The prices shown were for these models; if the list changed since, nothing is sent.
      if (S.mode !== 'compare' || C.sending || pending.ids.join('\n') !== C.ids.join('\n')) return;
      pending.ids.forEach((id) => S.confirmed.add(id));
      await cmpSend(pending.ids, pending.inputs);
      return;
    }
    // The price shown was for this model; if another one has loaded since, nothing is sent.
    if (!pending || !S.spec || S.spec.id !== pending.id) return;
    S.confirmed.add(pending.id);
    await send(pending.id, pending.input);
  });

  async function send(id, input) {
    const btn = $('#pgRun');
    S.sending = true;
    btn.disabled = true;
    $('#pgRunLabel').textContent = 'Sending…';
    $('#pgRunNote').textContent = '';
    const shown = S.spec && S.spec.id === id ? estimate() : null;
    const r = await call('/api/fal/run', { method: 'POST', body: { id, input, test: S.test, estimate: spendNote(shown) } });
    S.sending = false;
    btn.disabled = !(S.spec && S.spec.runnable) || S.offline;
    renderPrice();
    if (!r.ok) {
      const code = r.data && r.data.error;
      if (r.status === 0) { $('#pgRunNote').textContent = 'The engine is not answering. Is Agave Studio running?'; return; }
      if (code === 'not_test_mode') {
        // The studio restarted for real while this page thought it was in test mode: from now on runs cost money.
        await loadStatus();
        S.confirmed.clear();
        renderTemplates();
        loadPrice();
        cmpRefreshPrices();
        $('#pgRunNote').textContent = 'The studio is no longer in test mode, so runs now cost money. Check the price, then press Run again.';
        return;
      }
      if (code === 'missing_key') { S.keySet = false; $('#pgKey').hidden = false; }
      if (r.data && r.data.field) {
        setTab('form');
        showFieldError(r.data.field, fieldSay(r.data.field, code));
        const n = document.getElementById(`f_${r.data.field}`);
        if (n) n.focus();
      }
      $('#pgRunNote').textContent = code === 'not_runnable' && r.data.why ? r.data.why : say(code);
      return;
    }
    $('#pgRunNote').textContent = '';
    const job = r.data.job;
    upsertJob(job);
    show(job);
    follow(job.id);
    // One column (phones, narrow windows): the result is below the form, so bring it into view.
    if (matchMedia('(max-width: 1080px)').matches) $('#pgCanvas').scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
  }

  $('#pgRun').addEventListener('click', run);
  // Pressing a Run button does not blur the field you were in: a link typed into a file field would otherwise be added
  // on blur, the page would move under the pointer and the click would be lost. Run adds that link itself.
  for (const b of ['#pgRun', '#pgCmpRun']) $(b).addEventListener('mousedown', (e) => e.preventDefault());

  // ------------------------------------------------------------ results and history

  const isActive = (j) => j.status === 'queued' || j.status === 'running';
  const jobName = (j) => (j.request && j.request.name) || (j.model || '').replace(/^fal:/, '');
  const jobInput = (j) => j.request && j.request.playground ? j.request.playground : null;
  /** The words a run was given (its prompt or text), for its tile. */
  const jobWords = (j) => {
    const p = jobInput(j);
    const w = p && p.input ? (typeof p.input.prompt === 'string' ? p.input.prompt : typeof p.input.text === 'string' ? p.input.text : '') : '';
    return w.trim().slice(0, 200);
  };

  function upsertJob(job) {
    const i = S.jobs.findIndex((j) => j.id === job.id);
    if (i >= 0) S.jobs[i] = job; else S.jobs.unshift(job);
    S.jobs.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    renderHistory();
    if (S.shown && S.shown.id === job.id) { S.shown = job; renderCanvas(); }
    if (S.spec) renderPrice();
    if (S.mode === 'compare') renderCompare();
  }

  /** Follows one run until it ends: one request at a time, so an old reply can never land after a newer one. */
  function follow(id) {
    if (S.polls.has(id)) return;
    S.polls.set(id, true);
    syncTick();
    const step = async () => {
      const r = await call(`/api/jobs/${encodeURIComponent(id)}`);
      if (r.status === 404) { S.polls.delete(id); syncTick(); return; }
      if (r.ok && r.data && r.data.job) {
        const job = r.data.job;
        upsertJob(job);
        if (!isActive(job)) {
          S.polls.delete(id);
          syncTick();
          if (C.batch.includes(job.id)) cmpFinished(job);
          else announce(job.status === 'done' ? `${jobName(job)} is done.` : `${jobName(job)} failed.`);
          renderPrice();
          return;
        }
      }
      setTimeout(step, POLL_MS);
    };
    setTimeout(step, POLL_MS);
  }

  /** A one-second clock for the running timer, only while something runs and is on screen. */
  function syncTick() {
    const need = S.shown && isActive(S.shown);
    if (need && !S.tick) S.tick = setInterval(() => { const t = $('#pgElapsed'); if (t && S.shown) t.textContent = clock(Date.now() / 1000 - S.shown.created_at); }, 1000);
    if (!need && S.tick) { clearInterval(S.tick); S.tick = null; }
  }

  let viewer = null;
  function loadViewer() {
    if (!viewer) viewer = import('/vendor/model-viewer.min.js').catch(() => { viewer = null; });
    return viewer;
  }

  function bigMedia(out, job) {
    const alt = `Result of ${jobName(job)}`;
    if (out.kind === 'video') return video({ src: out.url, controls: true, autoplay: true, loop: true, playsinline: true, preload: 'metadata', 'aria-label': alt });
    if (out.kind === 'model' && !/\.glb$/i.test(out.url)) {
      return el('div', { class: 'pg-audio' }, [icon('3d'), el('p', {}, [`A ${out.url.split('.').pop().toUpperCase()} 3D file. The viewer shows GLB only; download it to open it.`])]);
    }
    if (out.kind === 'audio') return el('div', { class: 'pg-audio' }, [icon('audio'), el('audio', { src: out.url, controls: true, preload: 'metadata', 'aria-label': alt })]);
    if (out.kind === 'model') {
      loadViewer();
      return el('model-viewer', { src: out.url, alt, 'camera-controls': true, 'auto-rotate': true, 'shadow-intensity': '1', 'interaction-prompt': 'none' });
    }
    return el('img', { src: out.url, alt, decoding: 'async' });
  }

  /** When the result's top is off screen or low on it (a run opened from far down the list), scroll to it. */
  function bringResultIntoView() {
    const c = $('#pgCanvas');
    const top = c.getBoundingClientRect().top;
    if (top < 0 || top > innerHeight * 0.5) c.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
  }

  function show(job) {
    S.shown = job;
    S.shownOut = 0;
    renderCanvas();
    syncTick();
    document.querySelectorAll('.pg-hist').forEach((n) => n.setAttribute('aria-current', String(n.dataset.id === job.id)));
  }

  function renderCanvas() {
    const c = $('#pgCanvas');
    const job = S.shown;
    c.textContent = '';
    const bar = $('#pgResultBar');
    c.classList.toggle('empty', !job);
    if (!job) { c.append(EMPTY_CANVAS); bar.hidden = true; return; }
    if (isActive(job)) {
      c.append(el('div', { class: 'pg-running' }, [
        el('div', { class: 'ph' }),
        el('div', { class: 'pg-running-text' }, [
          el('p', { class: 'eyebrow' }, [job.status === 'queued' ? (S.test ? 'Waiting' : 'Waiting at fal') : 'Running']),
          el('p', { class: 'pg-clock', id: 'pgElapsed', role: 'timer', 'aria-label': 'Time so far' }, [clock(Date.now() / 1000 - job.created_at)]),
          el('p', { class: 'muted' }, [`${jobName(job)}. You can leave this page; the run keeps going and shows up here and in the studio.`]),
        ]),
      ]));
    } else if (job.status === 'failed') {
      c.append(el('div', { class: 'pg-failed' }, [
        el('p', { class: 'eyebrow' }, ['This run failed']),
        el('p', { class: 'pg-fail-msg' }, [job.error || 'No reason was given.']),
        el('p', { class: 'muted' }, [failAdvice(job)]),
      ]));
    } else {
      const outs = job.outputs || [];
      const out = outs[S.shownOut] || outs[0];
      if (out) c.append(el('div', { class: `pg-out-media kind-${out.kind}` }, [bigMedia(out, job)]));
      if (outs.length > 1) {
        const strip = el('div', { class: 'pg-out-strip', role: 'tablist', 'aria-label': 'Results of this run' });
        outs.forEach((o, i) => {
          const b = el('button', { type: 'button', role: 'tab', 'aria-selected': String(i === S.shownOut), 'aria-label': `Result ${i + 1}` }, [o.kind === 'image' ? el('img', { src: o.url, alt: '' }) : `${i + 1}`]);
          b.addEventListener('click', () => { S.shownOut = i; renderCanvas(); });
          strip.append(b);
        });
        c.append(strip);
      }
    }
    // The bar under the canvas: what ran, and what you can do with it.
    bar.hidden = false;
    const meta = $('#pgResultMeta');
    meta.textContent = '';
    meta.append(el('b', {}, [jobName(job)]), el('span', { class: 'muted' }, [` · ${ago(job.created_at)}`]));
    const pr = job.request && job.request.price;
    if (pr && num(pr.unit_price) && !pr.test) meta.append(el('span', { class: 'muted' }, [` · listed ${money(pr.unit_price)} per ${perUnit(pr.unit)}`]));
    if (job.prompt && job.prompt !== jobName(job)) meta.append(el('p', { class: 'pg-result-prompt' }, [job.prompt]));
    const out = (job.outputs || [])[S.shownOut] || (job.outputs || [])[0];
    const dl = $('#pgDownload');
    dl.hidden = !out;
    if (out) dl.href = out.url;
    $('#pgTweak').disabled = !jobInput(job);
    $('#pgResultCode').disabled = !jobInput(job);
  }

  /** What to do after a failed run, from what the error says. */
  function failAdvice(job) {
    const e = String(job.error || '');
    const code = (e.match(/provider HTTP (\d{3})/) || [])[1];
    if (code === '402' || /balance|credit|billing|payment/i.test(e)) return 'Your fal account needs credit or billing. Add it on fal, then run it again.';
    if (code === '401' || code === '403') return 'fal refused the key. Check your fal key, then run it again.';
    if (code === '429') return 'fal is limiting requests on your key. Wait a little, then run it again.';
    if (!jobInput(job)) return 'Run it again from the form.';
    if ((code && code.startsWith('4')) || e.startsWith('fal:')) return 'Press Tweak to load its settings, change what fal asked for, and run it again.';
    return 'Press Tweak to load its settings and run it again.';
  }

  function histThumb(job) {
    const out = (job.outputs || [])[0];
    if (isActive(job)) return el('span', { class: 'ph' });
    if (job.status === 'failed') return el('span', { class: 'pg-hist-x' }, ['!']);
    if (!out) return el('span', {});
    if (out.kind === 'image') return el('img', { src: out.url, alt: '', loading: 'lazy', decoding: 'async' });
    if (out.kind === 'video') return video({ src: out.url, preload: 'metadata', playsinline: true });
    return el('span', { class: 'pg-hist-kind' }, [out.kind === 'audio' ? 'audio' : out.kind === 'model' ? '3D' : 'file']);
  }

  function renderHistory() {
    const h = $('#pgHistory');
    h.textContent = '';
    const jobs = S.jobs.slice(0, 60);
    $('#pgHistNote').textContent = jobs.length ? 'They are in the studio gallery too.' : 'Nothing yet. Your Playground runs collect here.';
    for (const j of jobs) {
      const words = jobWords(j);
      const b = el('button', { type: 'button', class: `pg-hist ${j.status}`, 'data-id': j.id, 'aria-current': String(!!(S.shown && S.shown.id === j.id)) }, [
        el('span', { class: 'pg-hist-thumb', 'aria-hidden': 'true' }, [histThumb(j)]),
        el('span', { class: 'pg-hist-text' }, [
          el('span', { class: 'pg-hist-name' }, [jobName(j)]),
          words ? el('span', { class: 'pg-hist-words' }, [words]) : null,
          el('span', { class: 'pg-hist-when' }, [isActive(j) ? 'running' : j.status === 'failed' ? 'failed' : ago(j.created_at)]),
        ]),
      ]);
      b.addEventListener('click', (e) => {
        show(j);
        bringResultIntoView();
        // From the keyboard (Enter or Space), focus follows to the result so the next Tab starts there.
        if (e.detail === 0 && !$('#pgResultBar').hidden) $('#pgResultMeta').focus({ preventScroll: true });
      });
      h.append(el('div', { role: 'listitem' }, [b]));
    }
  }

  async function loadHistory() {
    const r = await call('/api/jobs');
    if (!r.ok || !r.data) return;
    S.jobs = (r.data.jobs || []).filter((j) => typeof j.model === 'string' && j.model.startsWith('fal:'));
    renderHistory();
    if (S.mode === 'compare') renderCompare();
    S.jobs.filter(isActive).forEach((j) => follow(j.id));
  }

  $('#pgTweak').addEventListener('click', () => {
    const p = S.shown && jobInput(S.shown);
    if (!p) return;
    if (S.tab === 'json') setTab('form', true);
    loadModel(p.id, p.input || {}).then(() => {
      $('#pgRunNote').textContent = 'Loaded the settings of that run. Change anything, then Run.';
      const first = document.querySelector('#pgFields textarea, #pgFields input, #pgFields button, #pgFields select');
      $('.pg-left').scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
      if (first) first.focus({ preventScroll: true });
    });
  });

  // ------------------------------------------------------------ templates

  async function loadTemplates() {
    try {
      const r = await fetch('/app/playground-templates.json');
      const d = r.ok ? await r.json() : null;
      S.templates = (d && Array.isArray(d.templates)) ? d.templates : [];
    } catch (_) { S.templates = []; }
    renderTemplates();
  }

  /** In test mode only the templates whose model is one of the seven samples. */
  const shownTemplates = () => S.templates.filter((t) => !S.test || (S.sampleIds && S.sampleIds.has(t.model)));

  function renderTemplates() {
    // The grid sits in the empty canvas, which is out of the page while a result is shown.
    const box = EMPTY_CANVAS.querySelector('#pgTemplates');
    box.textContent = '';
    for (const t of shownTemplates()) {
      const b = el('button', { type: 'button', class: 'pg-tpl' }, [
        icon(t.kind === 'video' ? 'video' : t.kind === 'audio' ? 'audio' : t.kind === '3d' ? '3d' : 'image'),
        el('span', { class: 'pg-tpl-text' }, [el('b', {}, [t.title]), el('span', {}, [t.blurb])]),
      ]);
      b.addEventListener('click', () => useTemplate(t));
      box.append(el('div', { role: 'listitem' }, [b]));
    }
    box.hidden = !box.children.length;
  }

  /** Loads the template's model with its words, and fal's example for any file it needs, so Run works at once. */
  async function useTemplate(t) {
    if (S.tab === 'json') setTab('form', true);
    // Words typed while the model loads are the person's and win over the template's, but only when they replaced
    // the old words: typed into an empty field or over all of it (select all, then type or paste). A keystroke in the
    // old model's example is not a new prompt.
    const box = document.querySelector('#pgFields [data-name="prompt"] :is(textarea, input), #pgFields [data-name="text"] :is(textarea, input)');
    const before = box ? box.value.trim() : '';
    let typed = null;
    let replaced = false;
    const words = (e) => e.target.closest && e.target.closest('[data-name="prompt"], [data-name="text"]');
    const onBefore = (e) => {
      if (!words(e)) return;
      const f = e.target;
      if (f.value === '' || (f.selectionStart === 0 && f.selectionEnd === f.value.length)) replaced = true;
    };
    const onType = (e) => {
      if (!words(e)) return;
      typed = e.target.value;
      if (typed === '') replaced = true;
    };
    const fields = $('#pgFields');
    fields.addEventListener('beforeinput', onBefore);
    fields.addEventListener('input', onType);
    try { await loadModel(t.model); } finally {
      fields.removeEventListener('beforeinput', onBefore);
      fields.removeEventListener('input', onType);
    }
    if (!S.spec || S.spec.id !== t.model) return;
    // Undone back to the old words is not a new prompt either.
    const own = replaced && typeof typed === 'string' && typed.trim() && typed.trim() !== before ? typed : null;
    const wordsField = typeof t.prompt === 'string' ? ['prompt', own || t.prompt] : typeof t.text === 'string' ? ['text', own || t.text] : null;
    if (wordsField && S.spec.fields.some((f) => f.name === wordsField[0])) {
      S.values[wordsField[0]] = wordsField[1];
      S.touched.add(wordsField[0]);
      S.exampled.delete(wordsField[0]);
    }
    // Only files the model needs: an optional one (an end frame, a mask) would change the result and the price.
    for (const f of S.spec.fields) {
      if (f.required && (f.kind === 'media' || f.kind === 'medialist') && !S.touched.has(f.name) && has(f.example)) {
        S.values[f.name] = f.example;
        S.touched.add(f.name);
        S.exampled.add(f.name);
      }
    }
    // The model may have filled fal's example in when it opened, before the loop above.
    const files = S.spec.fields.some((f) => (f.kind === 'media' || f.kind === 'medialist') && S.exampled.has(f.name));
    renderForm();
    renderPrice();
    if (S.tab === 'json') showJson();
    // What there is to change: the words, fal's example file. A template can say it better (one that needs your photo).
    const steps = [];
    if (S.spec.fields.some((f) => f.name === 'prompt' || f.name === 'text')) steps.push('change the words');
    if (files) steps.push("replace fal's example with your own file");
    const next = steps.length ? `${steps.join(' or ')}, then press Run.` : 'Press Run.';
    $('#pgRunNote').textContent = typeof t.note === 'string' ? `${t.title}: ${t.note}` : `${t.title}: ready to run. ${next.charAt(0).toUpperCase()}${next.slice(1)}`;
    const first = document.querySelector('#pgFields textarea, #pgFields .pg-drop');
    if (first) first.focus();
  }

  $('#pgShowTemplates').addEventListener('click', () => {
    S.shown = null;
    renderCanvas();
    syncTick();
    document.querySelectorAll('.pg-hist').forEach((n) => n.setAttribute('aria-current', 'false'));
    const first = $('#pgTemplates .pg-tpl');
    if (first) first.focus();
  });

  // ------------------------------------------------------------ code

  /** A JSON value as a Python literal (strings keep their text; only real true, false and null change). */
  function pyRepr(v, pad) {
    const inner = pad + '    ';
    if (v === null) return 'None';
    if (v === true) return 'True';
    if (v === false) return 'False';
    if (typeof v === 'string' || typeof v === 'number') return JSON.stringify(v);
    if (Array.isArray(v)) return v.length ? `[\n${v.map((x) => inner + pyRepr(x, inner)).join(',\n')},\n${pad}]` : '[]';
    const keys = Object.keys(v);
    return keys.length ? `{\n${keys.map((k) => `${inner}${JSON.stringify(k)}: ${pyRepr(v[k], inner)}`).join(',\n')},\n${pad}}` : '{}';
  }

  function codeFor(lang, id, input) {
    const local = JSON.stringify(input).includes('"/outputs/');
    const clean = JSON.parse(JSON.stringify(input, (k, v) => (typeof v === 'string' && v.startsWith('/outputs/') ? 'https://YOUR-FILE-URL' : v)));
    const body = JSON.stringify(clean, null, 2);
    let code;
    if (lang === 'js') {
      code = `import { fal } from "@fal-ai/client";\n\n// Reads FAL_KEY from the environment and waits for the result.\nconst result = await fal.subscribe("${id}", {\n  input: ${body.replace(/\n/g, '\n  ')},\n});\nconsole.log(result.data);\n`;
    } else if (lang === 'py') {
      code = `import fal_client\n\n# Reads FAL_KEY from the environment and waits for the result.\nresult = fal_client.subscribe(\n    "${id}",\n    arguments=${pyRepr(clean, '    ')},\n)\nprint(result)\n`;
    } else {
      const sq = body.replace(/'/g, "'\\''");
      code = `# Waits for the result and prints it (file URLs inside). For long jobs, use https://queue.fal.run/${id}\n# and then ask for the result with the request_id it returns.\ncurl -X POST "https://fal.run/${id}" \\\n  -H "Authorization: Key $FAL_KEY" \\\n  -H "Content-Type: application/json" \\\n  -d '${sq}'\n`;
    }
    return { code, local };
  }

  function openCode(id, input, title) {
    S.codeReq = { id, input };
    $('#pgCodeTitle').textContent = title;
    renderCode();
    $('#pgCode').showModal();
  }

  function renderCode() {
    if (!S.codeReq) return;
    const { code, local } = codeFor(S.codeLang, S.codeReq.id, S.codeReq.input);
    $('#pgCodeText').textContent = code;
    const setup = { py: 'Install the client with pip install fal-client. ', js: 'Install the client with npm install @fal-ai/client. ' }[S.codeLang] || '';
    $('#pgCodeNote').textContent = (local ? 'Files you uploaded here live only on this computer: put each one at a public link and replace https://YOUR-FILE-URL. ' : '') + setup + 'Set FAL_KEY in your shell first. Your key is never put into this code.';
    document.querySelectorAll('#pgCodeTabs [role=tab]').forEach((t) => t.setAttribute('aria-selected', String(t.dataset.lang === S.codeLang)));
  }

  document.querySelectorAll('#pgCodeTabs [role=tab]').forEach((t) => t.addEventListener('click', () => {
    S.codeLang = t.dataset.lang;
    try { localStorage.setItem('pg.codeLang', S.codeLang); } catch (_) { /* private window: this visit only */ }
    renderCode();
  }));
  $('#pgCodeBtn').addEventListener('click', () => {
    if (!S.spec) return;
    if (S.tab === 'json' && !applyJson()) return;
    openCode(S.spec.submit_path || S.spec.id, collect(), `Code for ${S.spec.name}, as set in the form`);
  });
  $('#pgResultCode').addEventListener('click', () => {
    const p = S.shown && jobInput(S.shown);
    if (!p) return;
    // The code uses fal's own path; it is the model's id unless fal's schema says otherwise (known for the model on screen).
    openCode(S.spec && S.spec.id === p.id ? (S.spec.submit_path || p.id) : p.id, p.input || {}, `Code for this run of ${jobName(S.shown)}`);
  });
  $('#pgCopyCode').addEventListener('click', async () => {
    const b = $('#pgCopyCode');
    try { await navigator.clipboard.writeText($('#pgCodeText').textContent); b.textContent = 'Copied'; } catch (_) { b.textContent = 'Select and copy'; }
    setTimeout(() => { b.textContent = 'Copy'; }, 1600);
  });

  // ------------------------------------------------------------ model picker

  function openPicker() {
    const d = $('#pgPicker');
    if (d.open) return;
    if (S.mode === 'compare' && C.ids.length >= MAX_CMP) { cmpNote('Compare takes up to four models. Remove one first.'); return; }
    $('#pgSearch').placeholder = S.test ? 'Search the seven test models: kling, voice, 3d…' : 'Search fal\'s models: kling, upscale, voice, logo…';
    $('#pgPickerTitle').textContent = S.mode === 'compare' ? 'Add a model to compare' : 'Choose a model';
    markAdded();
    d.showModal();
    $('#pgSearch').focus();
    if (!$('#pgList').children.length) searchModels(true);
  }

  function renderCats() {
    const box = $('#pgCats');
    box.textContent = '';
    const chip = (id, label) => {
      const b = el('button', { type: 'button', class: 'pg-cat', 'aria-pressed': String(S.cat === id) }, [label]);
      b.addEventListener('click', () => { S.cat = id; renderCats(); searchModels(true); });
      return b;
    };
    box.append(chip('', 'All'));
    S.cats.forEach((c) => box.append(chip(c.id, c.label)));
  }

  /** In Compare, the cards of models already in it say so and do nothing; the list is kept between opens. */
  function markAdded() {
    const cards = [...document.querySelectorAll('#pgList .pg-card[data-id]')];
    cards.forEach((b) => {
      const added = S.mode === 'compare' && C.ids.includes(b.dataset.id);
      b.classList.toggle('added', added);
      b.setAttribute('aria-label', b.dataset.label + (added ? ', already in the comparison' : ''));
      if (added) b.setAttribute('aria-disabled', 'true'); else b.removeAttribute('aria-disabled');
      const tag = b.querySelector('.pg-card-added');
      if (tag) tag.hidden = !added;
    });
    // Every card listed is already a column (and fal has no more of this kind): say so.
    const msg = $('#pgListMsg');
    const allAdded = cards.length && !S.listHasMore && cards.every((b) => b.classList.contains('added'));
    const sample = S.test ? ' Test mode has seven sample models; with your fal key, all of fal\'s models show here.' : '';
    if (allAdded) msg.textContent = `${cards.length === 1 ? 'This model is' : 'These models are'} already in the comparison. Pick another kind or search.${sample}`;
    else if (cards.length && msg.dataset.added) msg.textContent = '';
    msg.dataset.added = allAdded ? '1' : '';
  }

  function modelCard(m) {
    const label = `${m.name}, ${catLabel(m.category)}${m.makes_files ? '' : ', text only'}`;
    const b = el('button', { type: 'button', class: 'pg-card', 'aria-label': label, 'data-id': m.id, 'data-label': label, title: m.id }, [
      el('span', { class: 'pg-card-thumb', 'aria-hidden': 'true' }, [m.thumb ? el('img', { src: m.thumb, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' }) : el('span', { class: 'pg-card-ph' }, [catLabel(m.category)])]),
      el('span', { class: 'pg-card-body' }, [
        el('span', { class: 'pg-card-name' }, [m.name]),
        el('span', { class: 'pg-card-cat' }, [catLabel(m.category), m.makes_files ? null : ' · text only', el('span', { class: 'pg-card-added', hidden: true }, [' · added'])]),
        m.description ? el('span', { class: 'pg-card-desc' }, [m.description]) : null,
        el('span', { class: 'pg-card-id' }, [m.id]),
      ]),
    ]);
    b.addEventListener('click', () => {
      if (b.getAttribute('aria-disabled') === 'true') return; // already a column; the label says so
      $('#pgPicker').close();
      if (S.mode === 'compare') cmpAdd(m.id, m.name);
      else if (!S.spec || S.spec.id !== m.id) loadModel(m.id);
    });
    return el('div', { role: 'listitem' }, [b]);
  }

  async function searchModels(fresh) {
    const seq = ++S.listSeq;
    const list = $('#pgList');
    const msg = $('#pgListMsg');
    const more = $('#pgMoreModels');
    if (fresh) {
      S.cursor = null;
      list.textContent = '';
      for (let i = 0; i < 6; i++) list.append(el('div', { class: 'pg-card skel', 'aria-hidden': 'true' }));
    }
    msg.textContent = fresh ? 'Loading fal\'s models…' : '';
    more.disabled = true;
    const params = new URLSearchParams();
    if (S.q) params.set('q', S.q);
    if (S.cat) params.set('category', S.cat);
    if (!fresh && S.cursor) params.set('cursor', S.cursor);
    const r = await call(`/api/fal/models?${params}`);
    if (seq !== S.listSeq) return;
    if (fresh) list.textContent = '';
    more.disabled = false;
    if (!r.ok || !r.data) {
      msg.textContent = say(r.data && r.data.error);
      const retry = el('button', { type: 'button', class: 'pg-link-btn' }, ['Try again']);
      retry.addEventListener('click', () => searchModels(fresh));
      msg.append(' ', retry);
      more.hidden = true;
      return;
    }
    if (r.data.categories && !S.cats.length) { S.cats = r.data.categories; renderCats(); if (S.spec) renderModelHead(); renderCompare(); }
    const models = r.data.models || [];
    models.forEach((m) => list.append(modelCard(m)));
    S.cursor = r.data.next_cursor || null;
    S.listHasMore = !!r.data.has_more;
    more.hidden = !r.data.has_more;
    const sample = S.test ? ' Test mode has seven sample models; with your fal key, all of fal\'s models show here.' : '';
    const empty = (S.q ? `No fal model matches “${S.q}”.${/\s/.test(S.q) ? ' Try fewer words.' : ''}` : 'No models of this kind.') + sample;
    msg.textContent = list.querySelector('.pg-card[data-id]') ? '' : empty;
    msg.dataset.added = '';
    markAdded();
  }

  let searchTimer = null;
  $('#pgSearch').addEventListener('input', () => {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => { S.q = $('#pgSearch').value.trim(); searchModels(true); }, 280);
  });
  $('#pgSearch').addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); const first = $('#pgList .pg-card:not(.skel)'); if (first) first.focus(); }
    if (e.key === 'Enter') { e.preventDefault(); clearTimeout(searchTimer); S.q = $('#pgSearch').value.trim(); searchModels(true); }
  });
  $('#pgList').addEventListener('keydown', (e) => {
    const cards = [...document.querySelectorAll('#pgList .pg-card:not(.skel)')];
    const at = cards.indexOf(document.activeElement);
    if (at < 0) return;
    const cols = Math.max(1, Math.round($('#pgList').clientWidth / (cards[0].offsetWidth || 1)));
    const d = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: cols, ArrowUp: -cols }[e.key];
    if (!d) return;
    e.preventDefault();
    if (at + d < 0) { $('#pgSearch').focus(); return; }
    const next = cards[Math.min(cards.length - 1, at + d)];
    if (next) next.focus();
  });
  $('#pgMoreModels').addEventListener('click', () => searchModels(false));
  $('#pgModelBtn').addEventListener('click', openPicker);

  // Dialogs close on their close buttons and on a click outside the sheet.
  document.querySelectorAll('dialog').forEach((d) => {
    d.addEventListener('click', (e) => {
      if (e.target !== d) return;
      const r = d.getBoundingClientRect();
      if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) d.close();
    });
    d.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', () => d.close()));
  });
  $('#pgConfirm').addEventListener('close', () => { S.pendingRun = null; });

  // ------------------------------------------------------------ compare

  /* Compare: one prompt (and one picture, for models that need one) sent to up to four models side by side, with each
     model's price before Run all and its result, time and price after. Every other setting is the model's own default
     or fal's example, as when a model opens on its own; Open takes a column to its own page to run it alone. */
  const MAX_CMP = 4;
  const C = {
    ids: [], specs: new Map(), prices: new Map(), pstate: new Map(), pseq: new Map(), errs: new Map(),
    jobs: new Map(), // model id -> the job of its last Run all
    notes: new Map(), // model id -> why its run did not start: text, or {head, text} when it may have started
    sent: new Map(), // model id -> its price at the moment it was sent
    batch: [], told: false, // the jobs of the last Run all, for one summary when they all end
    image: null, sending: false, tick: null, cols: new Map(), started: false, addWrap: null,
  };
  S.mode = 'one';

  const wordsField = (spec) => spec.fields.find((f) => f.name === 'prompt') || spec.fields.find((f) => f.name === 'text');
  const imageField = (spec) => spec.fields.find((f) => f.required && (f.kind === 'media' || f.kind === 'medialist') && (f.media === 'image' || f.media === 'file'));
  const cmpNote = (msg) => { $('#pgCmpNote').textContent = msg || ''; };
  const cmpJob = (id) => { const jid = C.jobs.get(id); return jid ? S.jobs.find((j) => j.id === jid) || null : null; };
  const anyRunning = () => C.ids.some((id) => { const j = cmpJob(id); return j && isActive(j); });
  const took = (sec) => (sec < 1 ? 'under 1 s' : sec < 60 ? `${Math.round(sec)} s` : clock(sec));

  /** What Run all sends to one model: fal's example for what it needs, the shared words and picture over that. */
  function cmpInput(spec) {
    const input = {};
    for (const f of spec.fields) {
      const wanted = f.required || f.name === 'prompt' || f.name === 'text';
      if (wanted && !has(f.default) && has(f.example) && f.kind !== 'json') input[f.name] = f.example;
    }
    const text = $('#pgCmpPrompt').value.trim();
    const w = wordsField(spec);
    if (w && text) input[w.name] = text;
    const img = imageField(spec);
    if (img && C.image) input[img.name] = img.kind === 'medialist' ? [C.image] : C.image;
    return input;
  }

  function cmpEstimate(spec, input) {
    return estimateFor(spec, C.prices.get(spec.id), (name) => {
      if (name in input) return input[name];
      const f = spec.fields.find((x) => x.name === name);
      return f ? fieldDefault(f) : undefined;
    });
  }

  /** One model's price for these settings: the line in its column and what it adds to the total. */
  function cmpPriceOf(spec, input) {
    const st = C.pstate.get(spec.id);
    const p = C.prices.get(spec.id);
    const est = cmpEstimate(spec, input);
    if (st === 'loading' || !st) return { text: 'Asking fal for the price…', total: null, known: false };
    if (est) return { text: `${approx(est)}${money(est.total)} · ${est.how}`, total: est.total, about: !est.exact, word: approx(est), known: true };
    if (p && num(p.average)) return { text: `about ${money(p.average)} · fal's average run`, total: p.average, about: true, word: 'about ', known: true };
    if (p && num(p.unit_price)) return { text: `${money(p.unit_price)} per ${perUnit(p.unit)}`, total: null, known: false };
    if (st === 'nokey') return { text: 'Add your fal key to see the price.', total: null, known: false };
    return { text: 'fal did not give a price for this model.', total: null, known: false };
  }

  /** The word before a total: none when every price is exact, "up to" when every inexact one is a ceiling, else "about". */
  function totalWord(prices, unknown) {
    if (unknown > 0) return 'about ';
    const loose = prices.filter((p) => p.known && p.about);
    if (!loose.length) return '';
    return loose.every((p) => p.word === 'up to ') ? 'up to ' : 'about ';
  }

  /** The settings a column sends besides the words and the picture, in a few words. */
  const SHOWN_SETTINGS = ['duration', 'aspect_ratio', 'image_size', 'resolution', 'num_images'];
  function cmpSettings(spec, input) {
    const bits = [];
    for (const name of SHOWN_SETTINGS) {
      const f = spec.fields.find((x) => x.name === name);
      const v = !f ? null : name in input ? input[name] : fieldDefault(f);
      if (!has(v) || typeof v === 'object') continue;
      if (name === 'duration') bits.push(`${String(v).replace(/s$/, '')} s`);
      else if (name === 'num_images') bits.push(`${v} image${Number(v) === 1 ? '' : 's'}`);
      else bits.push(choiceLabel(v, f.choices));
    }
    return bits.length ? `Sends ${bits.join(', ')}` : '';
  }

  async function cmpLoad(id) {
    if (!C.specs.has(id)) {
      const r = await call(`/api/fal/model?id=${encodeURIComponent(id)}`);
      if (!C.ids.includes(id)) return;
      if (!r.ok || !r.data || !r.data.model) {
        C.errs.set(id, say(r.data && r.data.error));
        renderCompare();
        return;
      }
      C.specs.set(id, r.data.model);
    }
    renderCompare();
    cmpPrice(id);
  }

  async function cmpPrice(id) {
    if (!S.test && !S.keySet) { C.pstate.set(id, 'nokey'); renderCompare(); return; }
    if (['ok', 'loading'].includes(C.pstate.get(id))) return;
    const seq = (C.pseq.get(id) || 0) + 1;
    C.pseq.set(id, seq);
    C.pstate.set(id, 'loading');
    renderCompare();
    const r = await call(`/api/fal/price?id=${encodeURIComponent(id)}`);
    if (C.pseq.get(id) !== seq) return; // a newer ask answers instead
    if (r.ok && r.data && r.data.price) { C.prices.set(id, r.data.price); C.pstate.set(id, 'ok'); } else { C.prices.delete(id); C.pstate.set(id, 'none'); }
    if ($('#pgCmpNote').textContent.startsWith("fal's prices are still loading") && !C.ids.some((x) => C.pstate.get(x) === 'loading')) cmpNote('');
    renderCompare();
  }

  /** Asks fal again for every model's price: after a key is saved, or when the studio's mode turned out different. */
  function cmpRefreshPrices() {
    if (!C.started) return;
    C.pstate.clear();
    C.prices.clear();
    C.ids.forEach(cmpPrice);
  }

  /** This browser keeps the models, the words and the last results, so a reload finds Compare as it was. */
  function cmpSave() {
    store.set('pg.cmp', JSON.stringify(C.ids));
    const runs = {};
    for (const id of C.ids) if (C.jobs.has(id)) runs[id] = { job: C.jobs.get(id), sent: C.sent.get(id) || null };
    store.set('pg.cmp.runs', JSON.stringify(runs));
  }

  function cmpAdd(id, name) {
    if (C.sending) { cmpNote('Wait until Run all has sent every model.'); return; }
    if (C.ids.includes(id)) { cmpNote('That model is already here.'); return; }
    if (C.ids.length >= MAX_CMP) { cmpNote('Compare takes up to four models. Remove one first.'); return; }
    C.ids.push(id);
    cmpSave();
    cmpNote('');
    renderCompare();
    cmpLoad(id);
    const col = C.cols.get(id);
    if (col) col.node.focus();
    announce(`Added ${name || 'a model'}. ${C.ids.length} of ${MAX_CMP} models.`);
  }

  function cmpRemove(id) {
    if (C.sending) return; // its Remove button is off while Run all sends
    const at = C.ids.indexOf(id);
    const name = (C.specs.get(id) || {}).name || id;
    C.ids = C.ids.filter((x) => x !== id);
    C.batch = C.batch.filter((jid) => jid !== C.jobs.get(id));
    for (const m of [C.jobs, C.notes, C.sent, C.errs]) m.delete(id);
    const col = C.cols.get(id);
    if (col) col.node.remove();
    C.cols.delete(id);
    cmpSave();
    renderCompare();
    // Focus stays near: the column that moved into its place, else the one before, else Add.
    const next = C.cols.get(C.ids[at] || C.ids[at - 1]);
    const target = next ? next.node.querySelector('.pg-cmp-open') || next.node : $('#pgCmpCols .pg-cmp-add');
    (target || $('#pgCmpPrompt')).focus();
    announce(`Removed ${name}. ${C.ids.length} of ${MAX_CMP} models.`);
    // If it was the last one still running, the others' summary is due now.
    if (C.batch.length && !C.told) setTimeout(() => cmpFinished(null), 1200);
  }

  /** The first time Compare opens: the models from last time, else the one on screen. */
  function cmpStart() {
    if (!C.started) {
      C.started = true;
      let saved = [];
      try { saved = JSON.parse(store.get('pg.cmp') || '[]'); } catch (_) { saved = []; }
      C.ids = [...new Set((Array.isArray(saved) ? saved : []).filter((x) => typeof x === 'string' && x.length < 200))].slice(0, MAX_CMP);
      if (!C.ids.length && S.spec) C.ids = [S.spec.id];
      let runs = {};
      try { runs = JSON.parse(store.get('pg.cmp.runs') || '{}') || {}; } catch (_) { runs = {}; }
      for (const id of C.ids) {
        const r = runs[id];
        if (r && typeof r.job === 'string') { C.jobs.set(id, r.job); if (typeof r.sent === 'string') C.sent.set(id, r.sent); }
      }
      const words = store.get('pg.cmp.prompt');
      if (words) $('#pgCmpPrompt').value = words.slice(0, 20000);
      const pick = mediaPicker({
        id: 'pgCmpImage', many: false, kind: 'image', label: 'Picture', max: 1,
        get: () => C.image,
        set: (v) => { C.image = v || null; $('#pgCmpImage_e').textContent = ''; renderCompare(); },
        error: (msg) => { $('#pgCmpImage_e').textContent = msg; },
        busy: () => renderCompare(),
      });
      $('#pgCmpImageRow .pg-field-head').after(pick);
      C.ids.forEach(cmpLoad);
    }
    renderCompare();
  }

  /** A failed column's advice: Compare's button is Open, not Tweak. */
  function cmpAdvice(job) {
    return failAdvice(job)
      .replace(/^Press Tweak to load its settings/, 'Press Open to load its settings')
      .replace('Run it again from the form.', 'Press Open to run it again on its own.');
  }

  function cmpColumn(id) {
    const node = el('div', { class: 'pg-cmp-col glass', role: 'listitem', tabindex: '-1' });
    const col = { node, key: null, price: null, sets: null };
    C.cols.set(id, col);
    return col;
  }

  /** Redraws one column when what it shows changed. The price and settings lines change in place, and a result only
      redraws when the result itself changes, so videos and 3D views keep playing while you type. */
  function drawColumn(id) {
    const col = C.cols.get(id) || cmpColumn(id);
    const spec = C.specs.get(id);
    const job = cmpJob(id);
    const input = spec ? cmpInput(spec) : null;
    const ready = !!spec && !job && !C.notes.has(id);
    const key = JSON.stringify([!!spec, C.errs.get(id), C.notes.get(id), job && [job.id, job.status, job.error, (job.outputs || []).map((o) => o.url)],
      S.test, C.sending, C.sent.get(id), S.cats.length, ready && [Object.keys(input).sort(), C.image, !!$('#pgCmpPrompt').value.trim()]]);
    if (key !== col.key) {
      col.key = key;
      // Focus inside the column comes back to the same control (or the column) after the redraw.
      const had = col.node.contains(document.activeElement) ? document.activeElement : null;
      const which = had && ['.pg-cmp-x', '.pg-cmp-open', 'a.pg-act'].find((q) => had.matches(q));
      buildColumn(col, id, spec, job, input);
      if (had) {
        const t = which && col.node.querySelector(which);
        (t && !t.disabled ? t : col.node).focus({ preventScroll: true });
      }
    }
    if (spec && col.price) {
      const price = cmpPriceOf(spec, input);
      const text = S.test ? `Test, free · a real run: ${price.text}` : price.text;
      if (col.price.textContent !== text) col.price.textContent = text;
      const sets = cmpSettings(spec, input);
      if (col.sets.textContent !== sets) col.sets.textContent = sets;
    }
    return col;
  }

  function buildColumn(col, id, spec, job, input) {
    const n = col.node;
    n.textContent = '';
    col.price = null;
    col.sets = null;
    const name = spec ? spec.name : id;
    n.setAttribute('aria-label', name);
    const x = el('button', { type: 'button', class: 'pg-icon-btn pg-cmp-x', 'aria-label': `Remove ${name}`, title: 'Remove', disabled: C.sending }, [icon('close')]);
    x.addEventListener('click', () => cmpRemove(id));
    n.append(el('div', { class: 'pg-cmp-head' }, [
      el('span', { class: 'pg-thumb sm', 'aria-hidden': 'true' }, [spec && spec.thumb ? el('img', { src: spec.thumb, alt: '', loading: 'lazy', referrerpolicy: 'no-referrer' }) : icon(catIcon(spec && spec.category))]),
      el('span', { class: 'pg-cmp-title' }, [el('h2', { title: name }, [name]), el('span', { class: 'pg-model-meta', title: id }, [spec ? `${catLabel(spec.category)} · ${id}` : id])]),
      x,
    ]));
    if (C.errs.has(id)) {
      n.append(el('div', { class: 'pg-cmp-body' }, [el('p', { class: 'pg-cmp-msg err' }, [`${C.errs.get(id)} Remove it and pick another.`])]));
      return;
    }
    if (!spec) {
      n.append(el('div', { class: 'pg-cmp-body' }, [el('p', { class: 'pg-cmp-msg' }, ['Loading…'])]));
      return;
    }
    col.price = el('p', { class: 'pg-cmp-price' });
    col.sets = el('p', { class: 'pg-cmp-sets' });
    n.append(col.price, col.sets);
    const body = el('div', { class: 'pg-cmp-body' });
    const outs = job ? job.outputs || [] : [];
    if (job && isActive(job)) {
      body.append(el('div', { class: 'pg-cmp-wait' }, [
        el('p', { class: 'eyebrow' }, [job.status === 'queued' ? (S.test ? 'Waiting' : 'Waiting at fal') : 'Running']),
        el('p', { class: 'pg-cmp-clock', 'data-from': String(job.created_at), role: 'timer', 'aria-label': 'Time so far' }, [clock(Date.now() / 1000 - job.created_at)]),
      ]));
    } else if (job && job.status === 'failed') {
      body.append(el('div', { class: 'pg-cmp-msg' }, [el('p', { class: 'eyebrow err' }, ['This run failed']), el('p', {}, [job.error || 'No reason was given.']), el('p', { class: 'muted' }, [cmpAdvice(job)])]));
    } else if (job && outs.length) {
      body.classList.add('has-media', `kind-${outs[0].kind}`);
      body.append(bigMedia(outs[0], job));
    } else if (job) {
      body.append(el('p', { class: 'pg-cmp-msg' }, ['Done, but no file came back.']));
    } else if (C.notes.has(id)) {
      const note = C.notes.get(id);
      const head = typeof note === 'object' ? note.head : 'Not sent';
      body.append(el('div', { class: 'pg-cmp-msg' }, [el('p', { class: 'eyebrow err' }, [head]), el('p', {}, [typeof note === 'object' ? note.text : note])]));
    } else {
      const says = [];
      let state = 'Ready';
      const w = wordsField(spec);
      const img = imageField(spec);
      if (!w) says.push('Takes no words: it runs on the picture alone.');
      else if (!$('#pgCmpPrompt').value.trim()) says.push("Uses fal's example words until you write a prompt.");
      if (img && !C.image) {
        if (has(input[img.name])) says.push("Uses fal's example picture until you add one.");
        else { says.push('Add a picture above.'); state = 'Needs a picture'; }
      }
      const lacking = spec.fields.filter((f) => f.required && !(f.name in input) && f !== img);
      if (lacking.length) {
        says.push(`It needs ${lacking.map((f) => prettyTitle(f.title || f.name)).join(', ')}, which Compare can't send. Remove it, or Open it to run it alone.`);
        state = "Can't send this";
      }
      if (!spec.runnable) { says.push(spec.why_not || say('not_runnable')); state = "Can't run here"; }
      body.append(el('div', { class: 'pg-cmp-msg' }, [el('p', { class: `eyebrow${state === 'Ready' ? '' : ' err'}` }, [state]), ...says.map((t) => el('p', { class: 'muted' }, [t]))]));
    }
    n.append(body);
    const facts = [];
    if (job && job.status === 'done' && job.finished_at) facts.push(`took ${took(job.finished_at - job.created_at)}`);
    if (job && C.sent.has(id)) facts.push(C.sent.get(id));
    if (outs.length > 1) facts.push(`${outs.length} results: Open to see all`);
    const open = el('button', { type: 'button', class: 'pg-act pg-cmp-open', 'aria-label': `Open ${name} on its own` }, [icon('tweak'), 'Open']);
    open.addEventListener('click', () => cmpOpen(id));
    const acts = el('span', { class: 'pg-cmp-acts' }, [open]);
    if (outs[0]) acts.append(el('a', { class: 'pg-act', href: outs[0].url, download: '', 'aria-label': `Download the result of ${name}` }, [icon('down'), 'Download']));
    n.append(el('div', { class: 'pg-cmp-foot' }, [el('span', { class: 'pg-cmp-facts' }, [facts.join(' · ')]), acts]));
  }

  function renderCompare() {
    if (S.mode !== 'compare') return;
    const box = $('#pgCmpCols');
    C.ids.forEach((id, i) => {
      const { node } = drawColumn(id);
      if (box.children[i] !== node) box.insertBefore(node, box.children[i] || null);
    });
    for (const [id, col] of C.cols) if (!C.ids.includes(id)) { col.node.remove(); C.cols.delete(id); }
    if (!C.addWrap) {
      const add = el('button', { type: 'button', class: 'pg-cmp-add' }, [icon('plus'), el('span', {}, ['Add a model']), el('small', {}, ['Up to four fal models'])]);
      add.addEventListener('click', openPicker);
      C.addWrap = el('div', { role: 'listitem', class: 'pg-cmp-add-wrap' }, [add]);
    }
    if (C.ids.length < MAX_CMP) {
      if (box.lastElementChild !== C.addWrap) box.append(C.addWrap);
      const add = C.addWrap.firstElementChild;
      add.disabled = C.sending;
      add.setAttribute('aria-label', `Add a model, ${C.ids.length} of ${MAX_CMP} used`);
    } else {
      C.addWrap.remove();
    }
    // As many columns as there are models (and the Add tile), so up to four sit side by side.
    box.style.setProperty('--n', String(Math.max(1, Math.min(MAX_CMP, C.ids.length + (C.ids.length < MAX_CMP ? 1 : 0)))));
    // The shared picture, when a model here needs one.
    const needs = C.ids.map((id) => C.specs.get(id)).filter((s) => s && imageField(s));
    $('#pgCmpImageRow').hidden = !needs.length;
    if (!needs.length) {
      // No model needs a picture now: a refused link or upload has nothing left to say.
      $('#pgCmpImage_e').textContent = '';
      const l = $('#pgCmpImageRow .pg-link');
      if (l) l.value = '';
    }
    const names = needs.map((s) => s.name);
    const list = names.length > 1 ? `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}` : names[0] || '';
    $('#pgCmpImage_h').textContent = needs.length ? `For ${list}. Without one, ${needs.length === 1 ? 'it uses' : 'each uses'} fal's example picture where fal has one.` : '';
    renderCmpPrice();
    syncCmpTick();
  }

  function renderCmpPrice() {
    const specs = C.ids.map((id) => C.specs.get(id)).filter(Boolean);
    const waiting = C.ids.some((id) => C.pstate.get(id) === 'loading' || (!C.specs.has(id) && !C.errs.has(id)));
    const again = anyRunning() ? 'Run all again' : 'Run all';
    const parts = [];
    let label = again;
    if (!C.ids.length) {
      parts.push(el('span', { class: 'muted' }, ['Add a model to start. Each gets the same prompt.']));
    } else if (S.test) {
      label = `${again} · test, free`;
      parts.push(el('span', {}, ['Test mode: runs are free and fake.']));
    } else if (waiting) {
      parts.push(el('span', { class: 'muted' }, ['Asking fal for the prices…']));
    } else if (!S.keySet) {
      parts.push(el('span', { class: 'muted' }, ['Add your fal key to see the prices.']));
    } else {
      const prices = specs.map((s) => cmpPriceOf(s, cmpInput(s)));
      const total = prices.reduce((t, p) => t + (p.total || 0), 0);
      const unknown = prices.filter((p) => !p.known).length;
      const word = totalWord(prices, unknown);
      if (prices.some((p) => p.known)) {
        label = `${again} · ${word}${money(total)}`;
        parts.push(el('p', { class: 'pg-price-line' }, [el('b', {}, [`${word}${money(total)}`]), ` for ${specs.length === 1 ? 'one run' : `${specs.length} runs`}`,
          unknown ? el('span', { class: 'muted' }, [` · plus ${unknown === 1 ? 'one model' : `${unknown} models`} with no total`]) : null]));
        parts.push(el('p', { class: 'pg-price-note' }, ["fal's list prices, on your key. Each column shows its own."]));
      } else {
        label = `${again} · price unknown`;
        parts.push(el('span', { class: 'warn' }, ['fal did not give totals for these settings. Each column shows what fal lists.']));
      }
    }
    setLive($('#pgCmpPrice'), parts);
    if (!C.sending) $('#pgCmpRunLabel').textContent = label;
    $('#pgCmpRun').disabled = !C.ids.length || C.sending || S.offline || S.uploads > 0;
  }

  /** A one-second clock for the running columns, only while one runs and Compare is on screen. */
  function syncCmpTick() {
    const need = S.mode === 'compare' && anyRunning();
    if (need && !C.tick) {
      C.tick = setInterval(() => {
        document.querySelectorAll('#pgCmpCols .pg-cmp-clock').forEach((t) => { t.textContent = clock(Date.now() / 1000 - Number(t.dataset.from)); });
      }, 1000);
    }
    if (!need && C.tick) { clearInterval(C.tick); C.tick = null; }
  }

  /** One line when a run of the last Run all ends, and a summary once they all have, instead of a pile-up. */
  function cmpFinished(job) {
    const jobs = C.batch.map((jid) => S.jobs.find((j) => j.id === jid)).filter(Boolean);
    const ended = jobs.filter((j) => !isActive(j));
    if (ended.length < C.batch.length) {
      if (job) announce(`${jobName(job)} ${job.status === 'done' ? 'is done' : 'failed'}. ${ended.length} of ${C.batch.length} finished.`);
      return;
    }
    if (C.told || !C.batch.length) return;
    C.told = true;
    const failed = ended.filter((j) => j.status === 'failed').length;
    announce(C.batch.length === 1
      ? (failed ? 'The run failed.' : 'The run is done.')
      : `All ${C.batch.length} finished: ${ended.length - failed} done${failed ? `, ${failed} failed` : ''}.`);
  }

  function cmpConfirmText(specs, inputs) {
    const parts = [el('p', {}, [specs.length === 1 ? 'Run all sends one paid run to fal:' : `Run all sends ${specs.length} paid runs to fal, one per model:`])];
    const list = el('ul', { class: 'pg-cmp-confirm' });
    specs.forEach((s, i) => list.append(el('li', {}, [el('b', {}, [s.name]), `: ${cmpPriceOf(s, inputs[i]).text} `, el('a', { href: s.page, target: '_blank', rel: 'noopener' }, ['fal page ↗'])])));
    parts.push(list);
    const prices = specs.map((s, i) => cmpPriceOf(s, inputs[i]));
    const total = prices.reduce((t, p) => t + (p.total || 0), 0);
    const unknown = prices.filter((p) => !p.known).length;
    if (total > 0 && specs.length > 1) parts.push(el('p', {}, [unknown ? 'The ones with a total come to ' : 'Together that is ', totalWord(prices, unknown), el('b', {}, [money(total)]), unknown ? ', plus the others.' : '.']));
    parts.push(el('p', { class: 'muted' }, ["Options such as audio, resolution or quality can change fal's rate. It is paid to fal on your key. You are asked once per model."]));
    return parts;
  }

  async function cmpRun() {
    if (S.mode !== 'compare' || C.sending || !C.ids.length || S.offline) return;
    // A picture still on its way, or a link typed but not added, would leave fal's example picture in its place.
    if (S.uploads > 0) { cmpNote('The picture is still uploading. Press Run all when it shows.'); return; }
    if (!$('#pgCmpImageRow').hidden && commitLinks($('#pgCmpImageRow'))) { cmpNote('Fix the picture link first, or clear it.'); return; }
    if (!$('#pgCmpImageRow').hidden && !C.image && $('#pgCmpImage_e').textContent) {
      $('#pgCmpImage_e').textContent = '';
      cmpNote("The picture did not come in. Add it again, or press Run all again to use fal's example picture.");
      return;
    }
    if (matchMedia('(pointer: coarse)').matches && document.activeElement && document.activeElement.blur) document.activeElement.blur();
    const specs = C.ids.map((id) => C.specs.get(id));
    if (C.ids.some((id) => C.errs.has(id))) { cmpNote('A model here could not be loaded. Remove it first.'); return; }
    if (specs.some((s) => !s)) { cmpNote('A model is still loading. Press Run all again in a moment.'); return; }
    const off = specs.find((s) => !s.runnable);
    if (off) { cmpNote(`${off.name} cannot run here. Remove it first.`); return; }
    const inputs = specs.map(cmpInput);
    for (let i = 0; i < specs.length; i++) {
      const lacking = specs[i].fields.filter((f) => f.required && !(f.name in inputs[i]));
      if (lacking.length) {
        const img = imageField(specs[i]);
        cmpNote(img && lacking.includes(img)
          ? `${specs[i].name} needs a picture. Add one above, or remove it.`
          : `${specs[i].name} needs ${lacking.map((f) => prettyTitle(f.title || f.name)).join(', ')}, which Compare can't send. Remove it, or Open it to run it alone.`);
        return;
      }
    }
    if (!S.test && !S.keySet) { $('#pgKey').hidden = false; $('#pgKeyInput').focus(); cmpNote(ERRORS.missing_key); return; }
    if (!S.test) {
      // The price question is asked once per model, so never before fal's prices have had their chance to arrive.
      if (C.ids.some((id) => C.pstate.get(id) === 'loading')) { cmpNote("fal's prices are still loading. Press Run all again in a moment."); return; }
      if (C.ids.some((id) => !S.confirmed.has(id))) {
        $('#pgConfirmTitle').textContent = 'Check the prices first';
        $('#pgConfirmRunLabel').textContent = 'Run all';
        const box = $('#pgConfirmText');
        box.textContent = '';
        box.append(...cmpConfirmText(specs, inputs));
        S.pendingRun = { compare: true, ids: C.ids.slice(), inputs };
        $('#pgConfirm').showModal();
        return;
      }
    }
    await cmpSend(C.ids.slice(), inputs);
  }

  async function cmpSend(ids, inputs) {
    C.sending = true;
    $('#pgCmpRunLabel').textContent = 'Sending…';
    cmpNote('');
    // Every column starts fresh, so an old result never reads as one for the new prompt.
    for (const id of C.ids) for (const m of [C.notes, C.jobs, C.sent]) m.delete(id);
    C.batch = [];
    C.told = false;
    cmpSave();
    renderCompare();
    try {
      for (let i = 0; i < ids.length; i++) {
        const id = ids[i];
        if (!C.ids.includes(id)) continue;
        const spec = C.specs.get(id);
        const r = await call('/api/fal/run', { method: 'POST', body: { id, input: inputs[i], test: S.test, estimate: spendNote(spec && cmpEstimate(spec, inputs[i])) } });
        if (!r.ok) {
          const code = r.data && r.data.error;
          let stop = null;
          if (r.status === 0) {
            // The engine may have taken this one before the connection dropped: never call it "not sent".
            stop = 'The engine is not answering. Is Agave Studio running?';
            C.notes.set(id, { head: 'No answer', text: 'It may have started. Check Your runs on the One model page before running it again.' });
            i += 1;
          } else if (code === 'not_test_mode') {
            // The studio restarted for real while this page thought it was in test mode: from now on runs cost money.
            await loadStatus();
            S.confirmed.clear();
            renderTemplates();
            cmpRefreshPrices();
            loadPrice();
            stop = 'The studio is no longer in test mode, so runs now cost money. Check the prices, then press Run all again.';
          } else if (code === 'missing_key') {
            S.keySet = false;
            $('#pgKey').hidden = false;
            stop = ERRORS.missing_key;
          } else if (code === 'launch_token') {
            // Every other model would be refused the same way.
            stop = ERRORS.launch_token;
          }
          if (stop) {
            cmpNote(stop);
            for (const rest of ids.slice(i)) if (C.ids.includes(rest)) C.notes.set(rest, 'Not sent: Run all stopped before this model.');
            break;
          }
          const f = r.data && r.data.field && spec.fields.find((x) => x.name === r.data.field);
          C.notes.set(id, `${f ? `${prettyTitle(f.title || f.name)}: ` : ''}${code === 'not_runnable' && r.data.why ? r.data.why : say(code)}`);
          renderCompare();
          continue;
        }
        const job = r.data && r.data.job;
        if (!job || typeof job.id !== 'string') { C.notes.set(id, { head: 'No answer', text: 'The engine answered without a run; it may have started. Check Your runs on the One model page before running it again.' }); continue; }
        C.jobs.set(id, job.id);
        C.batch.push(job.id);
        const pr = cmpPriceOf(spec, inputs[i]);
        if (S.test) C.sent.set(id, 'test, free');
        else if (pr.total !== null) C.sent.set(id, `${pr.word || ''}${money(pr.total)}`);
        cmpSave();
        upsertJob(job);
        follow(job.id);
        renderCompare();
      }
    } finally {
      C.sending = false;
      cmpSave();
      renderCompare();
    }
    const first = $('#pgCmpCols .pg-cmp-col');
    if (first && matchMedia('(max-width: 1080px)').matches) first.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
  }

  /** A column on its own page: with the settings of its run, or of the run it would make. */
  async function cmpOpen(id) {
    const spec = C.specs.get(id);
    const job = cmpJob(id);
    const p = job && jobInput(job);
    setMode('one');
    await loadModel(id, p ? p.input : (spec ? cmpInput(spec) : undefined));
    if (!S.spec || S.spec.id !== id) return;
    if (job) show(job);
    $('#pgRunNote').textContent = job ? 'The settings of that run. Change anything, then Run.' : 'The settings Compare would send. Change anything, then Run.';
    $('#pgModelBtn').focus();
  }

  const SUB_ONE = $('#pgSub').textContent;
  const SUB_CMP = `One prompt, up to four of fal's models side by side, each with its price before you press Run all. ${MAC ? '⌘↵' : 'Ctrl+Enter'} runs all.`;

  /** `keepHash`: the address already says where to go (a typed #m=), so it is not rewritten. */
  function setMode(mode, keepHash = false) {
    if (S.mode === mode) return;
    S.mode = mode;
    const cmp = mode === 'compare';
    $('#pgModeOne').setAttribute('aria-selected', String(!cmp));
    $('#pgModeCmp').setAttribute('aria-selected', String(cmp));
    $('#pgModeOne').tabIndex = cmp ? -1 : 0;
    $('#pgModeCmp').tabIndex = cmp ? 0 : -1;
    $('#pgOne').hidden = cmp;
    $('#pgCompare').hidden = !cmp;
    $('#pgSub').textContent = cmp ? SUB_CMP : SUB_ONE;
    const skip = $('#pgSkip');
    skip.href = cmp ? '#pgCmpPrompt' : '#pgModelBtn';
    skip.textContent = cmp ? 'Skip to the prompt' : 'Skip to the model and its settings';
    store.set('pg.mode', mode);
    if (cmp) {
      if (!keepHash) history.replaceState(null, '', '#compare');
      cmpStart();
    } else if (S.spec && !keepHash) {
      history.replaceState(null, '', `#m=${encodeURIComponent(S.spec.id)}`);
    }
    syncCmpTick();
    syncTick();
  }

  $('#pgModeOne').addEventListener('click', () => setMode('one'));
  $('#pgModeCmp').addEventListener('click', () => setMode('compare'));
  arrowTabs($('.pg-modes'));
  $('#pgCmpRun').addEventListener('click', cmpRun);
  $('#pgCmpPrompt').addEventListener('input', () => { store.set('pg.cmp.prompt', $('#pgCmpPrompt').value); renderCompare(); });

  // ------------------------------------------------------------ keys

  document.addEventListener('keydown', (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement && document.activeElement.tagName);
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && !document.querySelector('dialog[open]')) {
      e.preventDefault();
      // The same guards as the buttons: never while a model loads, a run is being sent, or the key repeats.
      if (S.mode === 'compare') { if (!e.repeat && !C.sending && !$('#pgCmpRun').disabled) cmpRun(); return; }
      if (!e.repeat && !S.sending && !$('#pgRun').disabled) run();
      return;
    }
    if (e.key === '/' && !typing && !document.querySelector('dialog[open]')) { e.preventDefault(); openPicker(); }
  });

  // ------------------------------------------------------------ start

  function startModel() {
    const m = /^#m=(.+)$/.exec(location.hash);
    let id = null;
    if (m) { try { id = decodeURIComponent(m[1]); } catch (_) { id = null; } }
    return id || store.get('pg.model') || DEFAULT_MODEL;
  }

  // A model id typed or pasted into the address (#m=...) opens that model.
  // Only #m= and #compare change the page; the skip link's #target does not.
  window.addEventListener('hashchange', () => {
    if (location.hash === '#compare') { setMode('compare'); return; }
    if (!/^#m=/.test(location.hash)) return;
    const id = startModel();
    setMode('one', true);
    if (id && (!S.spec || S.spec.id !== id)) loadModel(id);
  });

  async function init() {
    const startHash = location.hash;
    if (!MAC) $('#pgModKey').textContent = 'Ctrl';
    await loadStatus();
    // Category names for the model header come with the first catalog page.
    call('/api/fal/models').then((r) => {
      if (r.ok && r.data && r.data.categories) { S.cats = r.data.categories; renderCats(); if (S.spec) renderModelHead(); renderCompare(); }
      // Test mode lists only the seven samples, so templates for other models are hidden there.
      if (r.ok && r.data && r.data.test) { S.sampleIds = new Set((r.data.models || []).map((m) => m.id)); renderTemplates(); }
    });
    loadTemplates();
    await loadModel(startModel());
    loadHistory();
    if (startHash === '#compare' || (!/^#m=/.test(startHash) && store.get('pg.mode') === 'compare')) setMode('compare');
  }

  init();
})();
