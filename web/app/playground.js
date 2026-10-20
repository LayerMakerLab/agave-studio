/* Agave Studio Playground: any fal model with its own form.
   The gateway compiles each model's fal schema into fields (/api/fal/model); this page draws them, sends only what
   you set (fal's defaults cover the rest), shows fal's price before Run, and follows the run in the job queue.
   Everything from fal (names, descriptions, errors) is set as text, never as HTML. */
(() => {
  'use strict';

  const DEFAULT_MODEL = 'fal-ai/flux/schnell';
  const MAX_FILE = 20 * 1024 * 1024; // the gateway sends local files inline to fal, up to 20 MB each
  const POLL_MS = 2000;
  const ACCEPT = { image: 'image/*', video: 'video/*', audio: 'audio/*', file: 'image/*,video/*,audio/*' };
  // fal's named image sizes (fal schema docs): width x height, for the megapixel price.
  const SIZES = {
    square_hd: [1024, 1024], square: [512, 512], portrait_4_3: [768, 1024], portrait_16_9: [576, 1024],
    landscape_4_3: [1024, 768], landscape_16_9: [1024, 576],
  };

  const $ = (s, root = document) => root.querySelector(s);
  const EMPTY_CANVAS = document.getElementById('pgCanvasEmpty');
  const S = {
    test: false, keySet: false, offline: false,
    spec: null, values: {}, touched: new Set(), exampled: new Set(), tab: 'form',
    price: null, priceState: 'none', confirmed: new Set(),
    jobs: [], polls: new Map(), shown: null, shownOut: 0, tick: null,
    cats: [], cat: '', q: '', cursor: null, listSeq: 0,
    codeLang: 'curl', codeReq: null, pendingRun: null, loadSeq: 0, sending: false,
    templates: [], sampleIds: null,
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
    if (v < 0.001) return `$${Number(v.toPrecision(2))}`;
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
  };
  const say = (code) => ERRORS[code] || 'Something went wrong. Try again.';

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
    x = x.toLowerCase().replace(/\b(cfg|fps|hd|lora|id|url|json|3d|sfx)\b/g, (m) => ({ lora: 'LoRA', '3d': '3D' }[m] || m.toUpperCase()));
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
    history.replaceState(null, '', `#m=${encodeURIComponent(spec.id)}`);
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
    $('#pgRun').disabled = !S.spec.runnable || S.offline;
  }

  function helpText(f) {
    const parts = [];
    if (f.description) parts.push(/[.!?]$/.test(f.description) ? f.description : `${f.description}.`);
    if (has(f.default) && f.kind !== 'longtext' && f.kind !== 'media') {
      parts.push(`Default: ${typeof f.default === 'object' ? JSON.stringify(f.default) : String(f.default)}.`);
    }
    if (f.limits) parts.push(f.limits);
    return parts.join(' ');
  }

  function fieldRow(f) {
    const id = `f_${f.name}`;
    const row = el('div', { class: `pg-field kind-${f.kind}`, 'data-name': f.name });
    const head = el('div', { class: 'pg-field-head' }, [
      el('label', { for: id, id: `${id}_l` }, [prettyTitle(f.title || f.name)]),
      f.required ? el('span', { class: 'pg-tag' }, ['needed']) : null,
      S.exampled.has(f.name) ? el('span', { class: 'pg-tag ex', title: "fal's example. Replace it with your own." }, ['example']) : null,
    ]);
    row.append(head, control(f, id));
    const help = helpText(f);
    if (help) row.append(el('p', { class: 'pg-help', id: `${id}_h` }, [help]));
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

  function textControl(f, id) {
    const long = f.kind === 'longtext';
    const v = S.touched.has(f.name) ? S.values[f.name] : '';
    const attrs = { id, 'aria-describedby': `${id}_h ${id}_e`, maxlength: f.max_len || null, spellcheck: long ? 'true' : 'false' };
    if (!S.touched.has(f.name) && has(f.default) && f.default !== '') attrs.placeholder = String(f.default);
    const n = long ? el('textarea', { ...attrs, rows: f.name === 'prompt' || f.name === 'text' ? 4 : 2 }) : el('input', { ...attrs, type: 'text' });
    n.value = typeof v === 'string' ? v : '';
    n.addEventListener('input', () => {
      setValue(f.name, n.value);
      markTouched(n.closest('.pg-field'));
      if (long) autosize(n);
    });
    if (long) requestAnimationFrame(() => autosize(n));
    return n;
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
    const box = el('input', { id, type: 'number', inputmode: f.kind === 'integer' ? 'numeric' : 'decimal', step, min: f.min, max: f.max, 'aria-describedby': `${id}_h ${id}_e` });
    const v = current(f);
    if (S.touched.has(f.name)) box.value = String(S.values[f.name]);
    else if (v !== null) box.placeholder = String(v);
    else if (f.name === 'seed') box.placeholder = 'random';
    const slider = f.min !== null && f.max !== null && f.max - f.min <= 100000
      ? el('input', { type: 'range', min: f.min, max: f.max, step: step === 'any' ? (f.max - f.min) / 100 : step, 'aria-labelledby': `${id}_l`, tabindex: '-1' })
      : null;
    if (slider) slider.value = String(v ?? f.min);
    const put = (raw, from) => {
      if (raw === '' || raw === null) { setValue(f.name, null); if (slider) slider.value = String(fieldDefault(f) ?? f.min); return; }
      let n = Number(raw);
      if (!isFinite(n)) return;
      if (f.kind === 'integer') n = Math.round(n);
      setValue(f.name, n);
      markTouched(wrap.closest('.pg-field'));
      if (from !== box) box.value = String(n);
      if (slider && from !== slider) slider.value = String(n);
    };
    box.addEventListener('input', () => {
      const n = Number(box.value);
      const out = box.value !== '' && ((num(f.min) && (f.min_strict ? n <= f.min : n < f.min)) || (num(f.max) && (f.max_strict ? n >= f.max : n > f.max)));
      if (out) { showFieldError(f.name, rangeText(f)); return; }
      put(box.value, box);
    });
    if (slider) {
      slider.addEventListener('input', () => put(slider.value, slider));
      wrap.append(slider);
    }
    wrap.append(box);
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

  /** landscape_16_9 -> Landscape 16:9, square_hd -> Square HD; anything else as fal wrote it. */
  function choiceLabel(c) {
    if (typeof c !== 'string') return String(c);
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
    const short = f.choices.length <= 6 && f.choices.every((c) => choiceLabel(c).length <= 14) && !f.custom_json;
    if (short) {
      const g = el('div', { class: 'pg-choices', role: 'radiogroup', id, 'aria-labelledby': `${id}_l`, 'aria-describedby': `${id}_h ${id}_e` });
      f.choices.forEach((c, i) => {
        const on = cur !== null && sameValue(c, cur);
        const label = f.name === 'duration' && /^\d+(\.\d+)?$/.test(String(c)) ? `${c} s` : choiceLabel(c);
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
    f.choices.forEach((c, i) => sel.append(el('option', { value: String(i) }, [choiceLabel(c)])));
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
    if (k === 'image') return el('img', { src: url, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' });
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
    const kind = f.media || 'file';
    const wrap = el('div', { class: 'pg-media', id, tabindex: '-1', 'aria-describedby': `${id}_h ${id}_e` });
    const list = () => {
      const v = S.touched.has(f.name) ? S.values[f.name] : null;
      if (many) return Array.isArray(v) ? v.slice() : [];
      return v ? [v] : [];
    };
    const save = (items) => setValue(f.name, many ? items : (items[0] || null));
    const draw = () => {
      wrap.textContent = '';
      const items = list();
      const tiles = el('div', { class: 'pg-tiles' });
      items.forEach((u, i) => {
        const rm = el('button', { type: 'button', class: 'pg-tile-x', 'aria-label': 'Remove this file' }, [icon('close')]);
        rm.addEventListener('click', () => { const next = list(); next.splice(i, 1); save(next); markTouched(wrap.closest('.pg-field')); draw(); });
        tiles.append(el('div', { class: 'pg-tile' }, [preview(u, kind), rm]));
      });
      const max = many ? Math.min(f.max_len || 16, 16) : 1;
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
            const r = await uploadFile(fl, kind);
            drop.disabled = false;
            if (r.error) { showFieldError(f.name, r.error); draw(); return; }
            next.push(r.url);
          }
          save(next);
          markTouched(wrap.closest('.pg-field'));
          draw();
        };
        drop.addEventListener('click', () => file.click());
        file.addEventListener('change', () => { if (file.files.length) take(file.files); });
        drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('over'); });
        drop.addEventListener('dragleave', () => drop.classList.remove('over'));
        drop.addEventListener('drop', (e) => { e.preventDefault(); drop.classList.remove('over'); if (e.dataTransfer.files.length) take(e.dataTransfer.files); });
        tiles.append(drop, file);
        const link = el('input', { type: 'url', class: 'pg-link', placeholder: 'or paste a link: https://…', 'aria-label': `${f.title}: paste a link`, spellcheck: 'false' });
        const addLink = () => {
          const u = link.value.trim();
          if (!u) return;
          if (!/^https:\/\/[^\s]+$/i.test(u)) { showFieldError(f.name, 'Links must start with https://.'); return; }
          const next = list();
          next.push(u);
          save(next);
          markTouched(wrap.closest('.pg-field'));
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

  function setTab(tab) {
    if (tab === S.tab || !S.spec) return;
    if (tab === 'form' && !applyJson()) return;
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
      if (at < 0 || !d) return;
      e.preventDefault();
      const next = tabs[(at + d + tabs.length) % tabs.length];
      next.focus();
      next.click();
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
    const p = S.price;
    if (!p || !num(p.unit_price)) return null;
    const unit = String(p.unit || '').toLowerCase().trim();
    const u = unit.replace(/s$/, '');
    const count = Number(fieldValue('num_images')) || 1;
    const sure = !S.spec.fields.some((f) => RATE_OPTIONS.includes(f.name));
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
      return { total: p.unit_price * mp * count, exact: false, how: `${count} × ${mp.toFixed(2)} MP` };
    }
    if (u === '1000 character') {
      const text = String(fieldValue('prompt') ?? fieldValue('text') ?? '');
      const k = Math.max(1, Math.ceil(text.length / 1000));
      return { total: p.unit_price * k, exact: false, how: `${text.length} characters` };
    }
    return null;
  }

  /** True while a run of the model on screen is still going (the button then says Run another). */
  const runningHere = () => !!S.spec && S.jobs.some((j) => isActive(j) && j.model === `fal:${S.spec.id}`);

  function priceSummary() {
    if (S.test) return { label: `${runningHere() ? 'Run another' : 'Run'} · test, free`, exact: true, est: estimate() };
    const est = estimate();
    const avg = S.price && S.price.average;
    if (est && est.exact) return { label: `Run · ${money(est.total)}`, exact: true, est, avg };
    if (est) return { label: `Run · about ${money(est.total)}`, exact: false, est, avg };
    if (num(avg)) return { label: `Run · about ${money(avg)}`, exact: false, avg };
    return { label: 'Run · check the price', exact: false };
  }

  function renderPrice() {
    const box = $('#pgPrice');
    box.textContent = '';
    if (!S.spec) return;
    const sum = priceSummary();
    const again = runningHere() && !S.test ? sum.label.replace(/^Run/, 'Run another') : sum.label;
    if (!S.sending) $('#pgRunLabel').textContent = S.spec.runnable ? again : 'Not runnable here';
    const p = S.price;
    if (S.test) {
      box.append(el('span', {}, ['Test mode: runs are free and fake.']));
      if (p && num(p.unit_price)) {
        const real = sum.est ? ` A real run: ${sum.est.how} → ${sum.est.exact ? '' : 'about '}${money(sum.est.total)}.` : '';
        box.append(el('span', { class: 'muted' }, [` fal lists ${money(p.unit_price)} per ${perUnit(p.unit)}.${real}`]));
      }
      return;
    }
    if (S.priceState === 'nokey') { box.append(el('span', { class: 'muted' }, ['Add your fal key to see the price.'])); return; }
    if (S.priceState === 'loading') { box.append(el('span', { class: 'muted' }, ['Asking fal for the price…'])); return; }
    if (!p) {
      box.append(el('span', { class: 'warn' }, ['fal did not give a price for this model. ']), el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['Check it on fal ↗']));
      return;
    }
    const line = el('p', { class: 'pg-price-line' });
    if (num(p.unit_price)) line.append(el('b', {}, [money(p.unit_price)]), ` per ${perUnit(p.unit)}`);
    if (sum.est) line.append(el('span', { class: 'muted' }, [` · ${sum.est.how} → ${sum.est.exact ? '' : 'about '}${money(sum.est.total)}`]));
    if (num(p.average)) line.append(el('span', { class: 'muted' }, [` · fal's average run ${money(p.average)}`]));
    box.append(line, el('p', { class: 'pg-price-note' }, ['fal\'s list price, on your key. Options such as audio, resolution or quality can change the rate: ', el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['full table on fal ↗'])]));
  }

  // ------------------------------------------------------------ run

  function confirmText(sum) {
    const p = S.price;
    const parts = [];
    if (p && num(p.unit_price)) parts.push(el('p', {}, ['fal lists ', el('b', {}, [money(p.unit_price)]), ` per ${perUnit(p.unit)} for ${S.spec.name}.`]));
    if (sum.est) parts.push(el('p', {}, [`For these settings (${sum.est.how}) that is ${sum.est.exact ? '' : 'about '}`, el('b', {}, [money(sum.est.total)]), '.']));
    if (p && num(p.average)) parts.push(el('p', {}, ['fal\'s average for one run of this model is ', el('b', {}, [money(p.average)]), '.']));
    if (!p) parts.push(el('p', {}, ['fal did not give a price for this model, so the studio cannot show one.']));
    parts.push(el('p', { class: 'muted' }, ['Options such as audio, resolution or quality can change fal\'s rate. ', el('a', { href: S.spec.page, target: '_blank', rel: 'noopener' }, ['See the full table on fal ↗']), ' It is paid to fal on your key. This question comes once per model.']));
    return parts;
  }

  async function run() {
    if (!S.spec || !S.spec.runnable) return;
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
    const r = await call('/api/fal/run', { method: 'POST', body: { id, input, test: S.test } });
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
        loadPrice();
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

  // ------------------------------------------------------------ results and history

  const isActive = (j) => j.status === 'queued' || j.status === 'running';
  const jobName = (j) => (j.request && j.request.name) || (j.model || '').replace(/^fal:/, '');
  const jobInput = (j) => j.request && j.request.playground ? j.request.playground : null;

  function upsertJob(job) {
    const i = S.jobs.findIndex((j) => j.id === job.id);
    if (i >= 0) S.jobs[i] = job; else S.jobs.unshift(job);
    S.jobs.sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    renderHistory();
    if (S.shown && S.shown.id === job.id) { S.shown = job; renderCanvas(); }
    if (S.spec) renderPrice();
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
          announce(job.status === 'done' ? `${jobName(job)} is done.` : `${jobName(job)} failed.`);
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
    if (out.kind === 'audio') return el('div', { class: 'pg-audio' }, [icon('audio'), el('audio', { src: out.url, controls: true, preload: 'metadata' })]);
    if (out.kind === 'model') {
      loadViewer();
      return el('model-viewer', { src: out.url, alt, 'camera-controls': true, 'auto-rotate': true, 'shadow-intensity': '1', 'interaction-prompt': 'none' });
    }
    return el('img', { src: out.url, alt, decoding: 'async' });
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
      const b = el('button', { type: 'button', class: `pg-hist ${j.status}`, 'data-id': j.id, 'aria-current': String(!!(S.shown && S.shown.id === j.id)) }, [
        el('span', { class: 'pg-hist-thumb' }, [histThumb(j)]),
        el('span', { class: 'pg-hist-text' }, [el('span', { class: 'pg-hist-name' }, [jobName(j)]), el('span', { class: 'pg-hist-when' }, [isActive(j) ? 'running' : j.status === 'failed' ? 'failed' : ago(j.created_at)])]),
      ]);
      b.addEventListener('click', () => show(j));
      h.append(el('div', { role: 'listitem' }, [b]));
    }
  }

  async function loadHistory() {
    const r = await call('/api/jobs');
    if (!r.ok || !r.data) return;
    S.jobs = (r.data.jobs || []).filter((j) => typeof j.model === 'string' && j.model.startsWith('fal:'));
    renderHistory();
    S.jobs.filter(isActive).forEach((j) => follow(j.id));
  }

  $('#pgTweak').addEventListener('click', () => {
    const p = S.shown && jobInput(S.shown);
    if (!p) return;
    if (S.tab === 'json') setTab('form');
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
    const box = $('#pgTemplates');
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
    if (S.tab === 'json') setTab('form');
    await loadModel(t.model);
    if (!S.spec || S.spec.id !== t.model) return;
    const words = typeof t.prompt === 'string' ? ['prompt', t.prompt] : typeof t.text === 'string' ? ['text', t.text] : null;
    if (words && S.spec.fields.some((f) => f.name === words[0])) {
      S.values[words[0]] = words[1];
      S.touched.add(words[0]);
      S.exampled.delete(words[0]);
    }
    for (const f of S.spec.fields) {
      if ((f.kind === 'media' || f.kind === 'medialist') && !S.touched.has(f.name) && has(f.example)) {
        S.values[f.name] = f.example;
        S.touched.add(f.name);
        S.exampled.add(f.name);
      }
    }
    renderForm();
    renderPrice();
    $('#pgRunNote').textContent = `${t.title}: ready to run. Change the words or replace fal's example with your own file.`;
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

  function openCode(id, input) {
    S.codeReq = { id, input };
    renderCode();
    $('#pgCode').showModal();
  }

  function renderCode() {
    if (!S.codeReq) return;
    const { code, local } = codeFor(S.codeLang, S.codeReq.id, S.codeReq.input);
    $('#pgCodeText').textContent = code;
    $('#pgCodeNote').textContent = (local ? 'Files you uploaded here live only on this computer: put each one at a public link and replace https://YOUR-FILE-URL. ' : '') + 'Set FAL_KEY in your shell first. Your key is never put into this code.';
    document.querySelectorAll('#pgCodeTabs [role=tab]').forEach((t) => t.setAttribute('aria-selected', String(t.dataset.lang === S.codeLang)));
  }

  document.querySelectorAll('#pgCodeTabs [role=tab]').forEach((t) => t.addEventListener('click', () => { S.codeLang = t.dataset.lang; renderCode(); }));
  $('#pgCodeBtn').addEventListener('click', () => {
    if (!S.spec) return;
    if (S.tab === 'json' && !applyJson()) return;
    openCode(S.spec.submit_path || S.spec.id, collect());
  });
  $('#pgResultCode').addEventListener('click', () => {
    const p = S.shown && jobInput(S.shown);
    if (!p) return;
    // The code uses fal's own path; it is the model's id unless fal's schema says otherwise (known for the model on screen).
    openCode(S.spec && S.spec.id === p.id ? (S.spec.submit_path || p.id) : p.id, p.input || {});
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
    $('#pgSearch').placeholder = S.test ? 'Search the seven test models: kling, voice, 3d…' : 'Search fal\'s models: kling, upscale, voice, logo…';
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

  function modelCard(m) {
    const label = `${m.name}, ${catLabel(m.category)}${m.makes_files ? '' : ', text only'}`;
    const b = el('button', { type: 'button', class: 'pg-card', 'aria-label': label, title: m.id }, [
      el('span', { class: 'pg-card-thumb', 'aria-hidden': 'true' }, [m.thumb ? el('img', { src: m.thumb, alt: '', loading: 'lazy', decoding: 'async', referrerpolicy: 'no-referrer' }) : el('span', { class: 'pg-card-ph' }, [catLabel(m.category)])]),
      el('span', { class: 'pg-card-body' }, [
        el('span', { class: 'pg-card-name' }, [m.name]),
        el('span', { class: 'pg-card-cat' }, [catLabel(m.category), m.makes_files ? null : ' · text only']),
        m.description ? el('span', { class: 'pg-card-desc' }, [m.description]) : null,
        el('span', { class: 'pg-card-id' }, [m.id]),
      ]),
    ]);
    b.addEventListener('click', () => { $('#pgPicker').close(); if (!S.spec || S.spec.id !== m.id) loadModel(m.id); });
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
    if (r.data.categories && !S.cats.length) { S.cats = r.data.categories; renderCats(); if (S.spec) renderModelHead(); }
    const models = r.data.models || [];
    models.forEach((m) => list.append(modelCard(m)));
    S.cursor = r.data.next_cursor || null;
    more.hidden = !r.data.has_more;
    let empty = S.q ? `No fal model matches “${S.q}”.${/\s/.test(S.q) ? ' Try fewer words.' : ''}` : 'No models of this kind.';
    if (S.test) empty += ' Test mode has seven sample models; with your fal key, all of fal\'s models show here.';
    msg.textContent = list.children.length ? '' : empty;
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

  // ------------------------------------------------------------ keys

  document.addEventListener('keydown', (e) => {
    const typing = /^(INPUT|TEXTAREA|SELECT)$/.test(document.activeElement && document.activeElement.tagName);
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter' && !document.querySelector('dialog[open]')) {
      e.preventDefault();
      // The same guards as the button: never while a model loads, a run is being sent, or the key repeats.
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
  window.addEventListener('hashchange', () => {
    const id = startModel();
    if (id && (!S.spec || S.spec.id !== id)) loadModel(id);
  });

  async function init() {
    await loadStatus();
    // Category names for the model header come with the first catalog page.
    call('/api/fal/models').then((r) => {
      if (r.ok && r.data && r.data.categories) { S.cats = r.data.categories; renderCats(); if (S.spec) renderModelHead(); }
      // Test mode lists only the seven samples, so templates for other models are hidden there.
      if (r.ok && r.data && r.data.test) { S.sampleIds = new Set((r.data.models || []).map((m) => m.id)); renderTemplates(); }
    });
    loadTemplates();
    await loadModel(startModel());
    loadHistory();
  }

  init();
})();
