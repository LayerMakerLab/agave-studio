/* Agave Studio workspace. Vanilla JS, no build step. Uses only the gateway API in docs/API.md. */
(() => {
  'use strict';

  const POLL_MS = 2500;
  const RETRY_MS = 5000;
  const DIM = '#77736c';
  const reduceMotion = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  // Rail modes. noRef / ref are the API `mode` values without and with a reference URL.
  const MODES = {
    image: { title: 'Image', sub: 'Text to image. Add a reference URL to switch to image to image.', modality: 'image', noRef: 't2i', ref: 'i2i', style: true, camera: false, refHint: 'Optional. Switches this to image to image.' },
    video: { title: 'Video', sub: 'Text to video. Add a reference URL to switch to image to video.', modality: 'video', noRef: 't2v', ref: 'i2v', style: true, camera: true, refHint: 'Optional. Switches this to image to video.' },
    camera: { title: 'Camera', sub: 'Video with a camera move and a look. Pick the move in the composer.', modality: 'video', noRef: 't2v', ref: 'i2v', style: true, camera: true, refHint: 'Optional. Switches this to image to video.' },
    audio: { title: 'Audio', sub: 'Text to speech.', modality: 'audio', noRef: 'tts', ref: 'tts', style: false, camera: false, noRefField: true },
    '3d': { title: '3D', sub: 'Turn 1 to 4 pictures of one object into a 3D model to spin, print (STL, 3MF) or view in AR (USDZ).', modality: '3d', noRef: 'i23d', ref: 'i23d', style: false, camera: false, needsRef: true, optionalPrompt: true, refHint: 'Optional. A picture URL, or add 1 to 4 pictures (front, side, back) below.' },
    characters: { title: 'Characters', sub: 'Image generation for characters you want to keep. Add a reference URL to start from one.', modality: 'image', noRef: 't2i', ref: 'i2i', style: true, camera: false, refHint: 'Optional. Starting image for the character.' },
  };

  // Plain words for the gateway's error codes.
  const ERRORS = {
    launch_token: 'This page lost its link to the studio. Reload the page, then try again.',
    adapter_not_implemented: 'This provider is not connected yet.',
    invalid_image_url: 'That link is not a usable https image link.',
    image_url_required: 'This model needs a picture to start from.',
    unsupported_mode: 'This model does not do that. Pick another model.',
    prompt_too_long: 'That prompt is too long. Shorten it.',
    unknown_camera: 'That camera move is not available.',
    unknown_style: 'That look is not available.',
    invalid_voice_id: 'That voice could not be used. Pick another.',
    voice_id_required: 'Pick a voice first.',
    unknown_model: 'That model is not in this studio.',
    prompt_required: 'Type a prompt first.',
    too_many_jobs: 'Too many jobs are running. Wait for some to finish.',
  };

  const PROMPT_HINT = {
    image: 'Describe the image', video: 'Describe the shot', camera: 'Describe the scene, then pick a move',
    audio: 'Type what the voice says',
    characters: 'Describe the character', '3d': 'Name the model (optional)',
  };

  const CHIPS = [
    { key: 'aspect', values: (m) => m.params && m.params.aspect, label: (v) => String(v), parse: (v) => String(v) },
    { key: 'duration', values: (m) => m.params && m.params.duration, label: (v) => `${v} s`, parse: (v) => Number(v) },
    { key: 'resolution', values: (m) => m.params && m.params.resolution, label: (v) => String(v), parse: (v) => String(v) },
    { key: 'audio', values: (m) => m.params && m.params.audio, label: (v) => (v ? 'On' : 'Off'), parse: (v) => v === 'true' },
  ];

  const STATUS_LABEL = { queued: 'Queued', running: 'Running', done: 'Done', failed: 'Failed' };

  const state = {
    mode: 'image',
    view: 'studio', // 'board' while the Board view shows (its drawing stays in the page when you leave it)
    voices: null,
    models: [],
    presets: { camera: [], styles: [] },
    keys: [],
    jobs: [],
    chips: { aspect: null, duration: null, resolution: null, audio: null },
    staticLoaded: false,
    want: null, // model id asked for by the link (#mode/model), applied once models load
    offline: false,
    busy: false,
    refOn: false,
    me: null,
    refs: [], // reference images in the composer tray (/outputs/... after upload)
    refKey: '', // ref count signature, so the model list re-filters when it changes
    elements: [], // saved Elements, called with @tag
    library: null, // prompt library entries (/app/library.json), loaded when the Library opens
    libCat: 'All',
    libModel: null, // model a library entry asked for; kept until the user picks another
    project: { style_prefix: '', still_first: false },
    edit: [], // Agave Edit timeline: { url, kind, from, to, len, caption }
    editFrame: 'original', // output frame: original, 16:9, 9:16, 1:1, 4:5
    editFill: false, // crop to fill the frame instead of fitting with bars
    editFollow: false, // with fill: the crop follows the subject
    editSel: -1, // selected clip
    ph: 0, // playhead, seconds into the edit
    autoCaps: 'off', // auto captions: off, simple, pop (word by word)
    transcripts: new Map(), // source url -> [{ t0, t1, text }] in source seconds
    wordFixes: [], // caption corrections: { from, to }
    undo: null, // timeline before the last Remove pauses
    undoAfter: '', // timeline right after it
    song: null, // { url, name } replacement soundtrack
    retried: new Set(), // failed job ids already run again this session
    audioPicked: false, // Audio on/off picked by you (else each model's default)
    lookOk: new Set(), // "picture|look" pairs that animate as they are (the look is on the picture, or you said so)
    editRev: null, // revision of the engine's saved timeline the local copy is based on
    editBase: new Set(), // clip urls of that saved timeline
  };

  const cards = new Map(); // job id -> { el, sig, fill }
  let timer = null;
  let refreshing = false;
  let again = false;
  let lastFocus = null;

  const $ = (sel) => document.querySelector(sel);
  const els = {
    form: $('#composer'),
    lookAsk: $('#lookAsk'),
    lookAskText: $('#lookAskText'),
    lookApply: $('#lookApply'),
    lookSkip: $('#lookSkip'),
    prompt: $('#prompt'),
    model: $('#model'),
    status: $('#status'),
    generate: $('#generate'),
    ref: $('#ref'),
    refWrap: $('#refWrap'),
    refHint: $('#refHint'),
    seed: $('#seed'),
    camera: $('#camera'),
    cameraHint: $('#cameraHint'),
    cameraWrap: $('#cameraWrap'),
    style: $('#style'),
    styleWrap: $('#styleWrap'),
    voiceWrap: $('#voiceWrap'),
    voice: $('#voice'),
    title: $('#modeTitle'),
    sub: $('#modeSub'),
    count: $('#jobCount'),
    banner: $('#banner'),
    empty: $('#empty'),
    emptyKey: $('#emptyKey'),
    gallery: $('#gallery'),
    dock: $('#dock'),
    settings: $('#settings'),
    scrim: $('#scrim'),
    keyList: $('#keyList'),
    spendIntro: $('#spendIntro'),
    spendRows: $('#spendRows'),
    spendActions: $('#spendActions'),
    spendRefresh: $('#spendRefresh'),
    spendNote: $('#spendNote'),
    closeSettings: $('#closeSettings'),
    shell: $('.shell'),
    cost: $('#cost'),
    recipe: $('#recipe'),
    recipeWrap: $('#recipeWrap'),
    countSel: $('#count'),
    tray: $('#tray'),
    elRow: $('#elRow'),
    addRef: $('#addRef'),
    addWrap: $('#addWrap'),
    fileIn: $('#fileIn'),
    retryAll: $('#retryAll'),
    library: $('#library'),
    prompts: $('#prompts'),
    libCats: $('#libCats'),
    libGrid: $('#libGrid'),
    editor: $('#editor'),
    elForm: $('#elementForm'),
    elTag: $('#elTag'),
    elName: $('#elName'),
    elKind: $('#elKind'),
    elNote: $('#elNote'),
    elList: $('#elList'),
    elNewImgs: $('#elNewImgs'),
    keepSound: $('#keepSound'),
    addSong: $('#addSong'),
    songIn: $('#songIn'),
    songName: $('#songName'),
    clearSong: $('#clearSong'),
    joinBtn: $('#joinBtn'),
    pv: $('#pv'),
    pvVideo: $('#pvVideo'),
    pvImg: $('#pvImg'),
    pvCap: $('#pvCap'),
    pvEmpty: $('#pvEmpty'),
    pvPlay: $('#pvPlay'),
    pvTime: $('#pvTime'),
    frameChips: $('#frameChips'),
    fitChips: $('#fitChips'),
    timeline: $('#timeline'),
    tlHead: $('#tlHead'),
    splitBtn: $('#splitBtn'),
    addClipBtn: $('#addClipBtn'),
    clipIn: $('#clipIn'),
    tlNote: $('#tlNote'),
    inspector: $('#inspector'),
    pvAuto: $('#pvAuto'),
    pausesBtn: $('#pausesBtn'),
    undoBtn: $('#undoBtn'),
    autoCapChips: $('#autoCapChips'),
    capNote: $('#capNote'),
    fixForm: $('#fixForm'),
    fixFrom: $('#fixFrom'),
    fixTo: $('#fixTo'),
    fixList: $('#fixList'),
    cleanVoice: $('#cleanVoice'),
    loudness: $('#loudness'),
    forYouTube: $('#forYouTube'),
    forVertical: $('#forVertical'),
    editNote: $('#editNote'),
    editCount: $('#editCount'),
    projectForm: $('#projectForm'),
    stylePrefix: $('#stylePrefix'),
    stillFirst: $('#stillFirst'),
    projectNote: $('#projectNote'),
    pipeline: $('#pipeline'),
    pipelineTitle: $('#pipelineTitle'),
    pipeBody: $('#pipeBody'),
    pipeFile: $('#pipeFile'),
    pipeGrid: $('#pipeGrid'),
    runList: $('#runList'),
    runChip: $('#runChip'),
    runChipText: $('#runChipText'),
    testBanner: $('#testBanner'),
    board: $('#board'),
    boardBtn: $('#boardBtn'),
  };

  // ---------- helpers ----------

  // Builds a node. Text is always set with textContent, never innerHTML.
  function el(tag, attrs, kids) {
    const node = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs || {})) {
      if (v === null || v === undefined || v === false) continue;
      if (k === 'class') node.className = v;
      else if (v === true) node.setAttribute(k, '');
      else node.setAttribute(k, String(v));
    }
    for (const kid of [].concat(kids || [])) {
      if (kid === null || kid === undefined || kid === false) continue;
      node.append(kid instanceof Node ? kid : document.createTextNode(String(kid)));
    }
    return node;
  }

  const opt = (value, label, disabled) => el('option', { value, disabled }, [label]);

  async function call(path, opts = {}) {
    const init = { method: opts.method || 'GET', headers: {}, cache: 'no-store' };
    if (opts.body !== undefined) {
      init.headers['Content-Type'] = 'application/json';
      init.body = JSON.stringify(opts.body);
    }
    const res = await fetch(path, init);
    let data = null;
    try { data = await res.json(); } catch (_) { data = null; }
    if (res.status === 403 && data && data.error === 'launch_token') showStale();
    return { ok: res.ok, status: res.status, data };
  }

  /** The engine restarted: this page's launch cookie is old, and only a reload gets the new one. */
  function showStale() {
    const b = document.getElementById('staleBanner');
    if (!b || !b.hidden) return;
    b.hidden = false;
    b.scrollIntoView({ block: 'nearest' });
  }
  document.getElementById('staleReload')?.addEventListener('click', () => location.reload());

  function setOffline(v) {
    const changed = state.offline !== v;
    state.offline = v;
    els.banner.hidden = !v;
    if (changed && !state.staticLoaded) renderModels(); // swap "Loading models..." for "Engine offline"
  }

  function setStatus(msg, tone) {
    els.status.textContent = msg;
    els.status.className = 'composer-status' + (tone ? ' ' + tone : '');
  }

  const isActive = (j) => j.status === 'queued' || j.status === 'running';
  const modelName = (id) => {
    const m = state.models.find((x) => x.id === id);
    return m ? m.name : id;
  };
  // Playground runs (fal:<endpoint>) carry the fal model's own name.
  const jobModelName = (job) => (typeof job.model === 'string' && job.model.startsWith('fal:') && job.request && typeof job.request.name === 'string'
    ? `${job.request.name} · Playground` : modelName(job.model));
  const currentMode = () => MODES[state.mode];
  // @tags of saved Elements in the prompt (same rule as the engine: not right after a letter, so emails are skipped).
  const TAG_RE = /(^|[^A-Za-z0-9])@([A-Za-z0-9][A-Za-z0-9_-]*)/g;
  function taggedElements() {
    const out = [];
    for (const m of els.prompt.value.matchAll(TAG_RE)) {
      const tag = m[2].toLowerCase().replace(/[-_]+$/, '');
      const e = state.elements.find((x) => x.tag === tag);
      if (e && !out.includes(e)) out.push(e);
    }
    return out;
  }
  // The composer's own images: the URL field, then the tray.
  function ownRefs() {
    const list = [];
    const url = els.ref.value.trim();
    if (!currentMode().noRefField && url) list.push(url);
    for (const r of state.refs) if (!list.includes(r)) list.push(r);
    return list;
  }
  // Everything the engine will send as references, including the images of @tagged Elements.
  function allRefs() {
    const list = ownRefs();
    for (const e of taggedElements()) for (const i of e.images) if (!list.includes(i)) list.push(i);
    return list;
  }
  const refActive = () => !currentMode().noRefField && allRefs().length > 0;
  const effectiveMode = () => (refActive() ? currentMode().ref : currentMode().noRef);
  const selectedModel = () => state.models.find((m) => m.id === els.model.value) || null;
  // How many pictures a model gets as references. A model that takes Elements beside its start frame
  // (Kling 3.0) gets the @tagged Elements' pictures as Elements, so only the composer's own count (engine rule).
  function refCountFor(m) {
    if (!refActive()) return 0;
    const own = ownRefs().length;
    if (own > 0 && (m.max_elements || 0) > 0 && effectiveMode() === 'i2v') return own;
    return allRefs().length;
  }
  const eligibleModels = () => {
    const md = currentMode();
    const mode = effectiveMode();
    return state.models.filter((m) => {
      const n = refCountFor(m);
      const tags = (m.max_elements || 0) > 0 && n < allRefs().length ? taggedElements().filter((e) => e.images.length).length : 0;
      return (md.modality === null || m.modality === md.modality) &&
        Array.isArray(m.modes) && m.modes.includes(mode) &&
        (n <= 1 || (m.max_refs || 0) >= n) && tags <= (m.max_elements || 0);
    });
  };
  // Re-filters the model list when references appear, go away, or pass one.
  function refKeyNow() {
    const n = refActive() ? allRefs().length : 0;
    return `${n > 0}:${n > 1 ? n : 1}`;
  }
  function syncRefMode() {
    const key = refKeyNow();
    if (key !== state.refKey) {
      state.refKey = key;
      state.refOn = refActive();
      renderModels();
      writeHash();
    } else {
      renderCost();
    }
  }
  const paint = (fill, progress) => {
    if (fill && typeof progress === 'number') {
      fill.style.width = `${Math.round(Math.min(1, Math.max(0, progress)) * 100)}%`;
    }
  };

  // ---------- composer rendering ----------

  function renderModels() {
    const prev = els.model.value;
    const list = eligibleModels();
    els.model.textContent = '';
    if (!list.length) {
      const msg = !state.staticLoaded
        ? (state.offline ? 'Engine offline' : 'Loading models...')
        : state.models.some((m) => m.modes && (m.modes.includes(currentMode().noRef) || m.modes.includes(currentMode().ref)))
          ? 'No model for this mode'
          : 'Coming soon';
      els.model.append(opt('', msg)); // a disabled option would render as a blank chip
      els.model.disabled = true;
    } else {
      els.model.disabled = false;
      for (const m of list) {
        const o = opt(m.id, m.ready ? `${m.name} · ${m.vendor}` : `${m.name} · needs ${m.key}`);
        if (!m.ready) o.style.color = DIM;
        els.model.append(o);
      }
      const asked = list.find((m) => m.id === state.want) || list.find((m) => m.id === state.libModel);
      if (state.staticLoaded) state.want = null;
      const keep = asked || list.find((m) => m.id === prev);
      const fallback = list.find((m) => m.ready) || list[0];
      els.model.value = (keep || fallback).id;
    }
    if (!state.busy) els.generate.disabled = state.staticLoaded && !list.length;
    renderChips();
    renderFlags();
    renderCost();
  }

  function renderChips() {
    const m = selectedModel();
    for (const c of CHIPS) {
      const sel = $('#' + c.key);
      const wrap = sel.parentElement;
      const values = (m && c.values(m)) || [];
      sel.textContent = '';
      if (!values.length) {
        wrap.hidden = true;
        state.chips[c.key] = null;
        continue;
      }
      // The first listed value is the model's default; lengths are shown in order.
      const shown = c.key === 'duration' ? [...values].sort((a, b) => a - b) : values;
      shown.forEach((v) => sel.append(opt(String(v), c.label(v))));
      // Audio follows each model's own default until you pick it yourself (prices differ by model).
      const cur = c.key === 'audio' && !state.audioPicked ? undefined : state.chips[c.key];
      const pick = values.find((v) => String(v) === String(cur)) ?? values[0];
      sel.value = String(pick);
      state.chips[c.key] = c.parse(sel.value);
      wrap.hidden = false;
    }
  }

  // On a model that takes an end picture (Kling 3.0 image to video), the picked move is keyframed (the engine sends
  // where the move ends) or words only. Null when there is no move or the model takes no end picture. A view move's
  // edit runs on the video's own fal key, or the engine sends the move as words (same rule as the engine).
  function cameraPlan() {
    return els.cameraWrap.hidden ? null : cameraPlanFor(selectedModel(), els.camera.value, effectiveMode());
  }
  function cameraPlanFor(m, id, mode) {
    if (!id || !m || !m.end_frame || mode !== 'i2v') return null;
    const p = state.presets.camera.find((c) => c.id === id);
    if (!p) return null;
    const plan = p.keyframe || { kind: 'words' };
    if (plan.kind === 'view') {
      const em = state.models.find((x) => x.id === plan.model);
      if (!em || em.key !== m.key || em.provider !== 'fal' || m.provider !== 'fal') return { kind: 'words' };
    }
    return plan;
  }
  // A view move's end picture is an image edit on the same key, priced from its model.
  function keyframeCents(plan) {
    const em = plan && plan.kind === 'view' ? state.models.find((x) => x.id === plan.model) : null;
    return em && em.price ? Math.max(1, Math.ceil(Math.round(em.price.usd * 1e6) / 1e4)) : 0;
  }
  function renderCameraHint() {
    const plan = cameraPlan();
    els.cameraHint.hidden = !plan;
    if (!plan) return;
    const name = (els.camera.selectedOptions[0] || {}).textContent || 'This move';
    if (plan.kind === 'crop') {
      els.cameraHint.textContent = `${name} is keyframed: Agave also sends where the move ends, a crop of your picture, so the camera should make it.`;
    } else if (plan.kind === 'view') {
      const em = state.models.find((x) => x.id === plan.model);
      els.cameraHint.textContent = `${name} is keyframed: ${em ? em.name : 'an image edit'} first makes where the move ends (${keyframeCents(plan)}¢, in the price, charged even if the video then fails), and the video should move to it.`;
    } else {
      els.cameraHint.textContent = `${name} goes as words only. This model often ignores camera words, so the move may not happen.`;
    }
  }

  // US cents for one job with the current model and options, from the provider list price (rounded up).
  function jobCents() {
    const c = state.chips;
    return priceCents(selectedModel(), c.duration, c.resolution, c.audio, cameraPlan());
  }
  // The same price for a request already sent (Run again), or null when the model lists no dollar price. It applies
  // the engine's own rules: an option the request left out is the model's first listed value, and image to video
  // always has a start picture (yours or an @Element's), so a keyframed move's edit is always part of it.
  function requestCents(req) {
    const m = req && state.models.find((x) => x.id === req.model);
    if (!m) return null;
    const pick = (v, key) => (v === null || v === undefined ? ((m.params && m.params[key]) || [])[0] : v);
    const plan = (m.max_refs || 0) > 0 ? null : cameraPlanFor(m, req.camera, req.mode);
    return priceCents(m, pick(req.duration, 'duration'), pick(req.resolution, 'resolution'), pick(req.audio, 'audio'), plan);
  }
  function priceCents(m, duration, resolution, audioOn, plan) {
    const p = m && m.price;
    if (!p) return null;
    const audio = audioOn === null || audioOn === undefined ? null : (audioOn ? 'on' : 'off');
    const rate = (audio && p.by_audio && p.by_audio[audio]) || (p.by_resolution && p.by_resolution[resolution]) || p.usd;
    const units = p.per === 'second' ? duration : 1;
    if (!units) return null;
    const micro = Math.round(rate * units * 1e6);
    return Math.max(1, Math.ceil(micro / 1e4)) + keyframeCents(plan);
  }
  // What a generate request sends for Agave's spend log: the list-price estimate the studio shows (rounded up to the
  // cent, so "about"), or nothing when the model lists no dollar price.
  function spendNote(req) {
    const cents = requestCents(req);
    const m = state.models.find((x) => x.id === req.model);
    if (cents === null || !m || !m.price) return undefined;
    const dur = req.duration ?? ((m.params && m.params.duration) || [])[0];
    const how = m.price.per === 'second' ? `${dur} s` : '1 image';
    return { usd: cents / 100, kind: 'about', how };
  }
  function priceNote(price) {
    return price === 'price not listed' ? 'the provider lists no price for it' : `${price}, provider list price`;
  }
  // "≈ $0.37" for a list of requests, "about N Vendor credits", or 'price not listed'.
  function requestsPrice(reqs) {
    const cents = reqs.map(requestCents);
    if (cents.every((c) => c !== null)) return `≈ $${(cents.reduce((a, b) => a + b, 0) / 100).toFixed(2)}`;
    const ms = reqs.map((r) => state.models.find((x) => x.id === r.model));
    if (ms.every((m) => m && m.provider_credits && m.vendor === ms[0].vendor)) {
      return `about ${ms.reduce((a, m) => a + m.provider_credits, 0)} ${ms[0].vendor} credits`;
    }
    return 'price not listed';
  }

  // Cost before every paid step: the provider list price on your own key.
  function renderCost() {
    renderCameraHint();
    const n = Number(els.countSel.value) || 1;
    const one = jobCents();
    const c = one === null ? null : one * n;
    const m = selectedModel();
    if (c === null && m && m.provider_credits) {
      const pc = m.provider_credits * n;
      els.cost.hidden = false;
      els.cost.textContent = `${pc} ${m.vendor} cr`;
      els.generate.title = `About ${pc} ${m.vendor} credits on your own key (${m.vendor}'s price list; no dollar price is published)`;
      return;
    }
    els.cost.hidden = c === null;
    if (c === null) {
      els.cost.textContent = '';
      els.generate.title = '';
    } else {
      els.cost.textContent = `≈ $${(c / 100).toFixed(2)}`;
      els.generate.title = `About $${(c / 100).toFixed(2)} on your own key (provider list price, rounded up to the cent)`;
    }
  }

  function renderFlags() {
    const md = currentMode();
    els.refWrap.hidden = !!md.noRefField;
    els.refHint.textContent = md.refHint || '';
    els.cameraWrap.hidden = !md.camera;
    els.styleWrap.hidden = !md.style;
    els.voiceWrap.hidden = md.modality !== 'audio';
    if (!els.voiceWrap.hidden) loadVoices();
    els.recipeWrap.hidden = md.modality !== 'image';
    if (els.recipeWrap.hidden) els.recipe.value = '';
    els.addWrap.hidden = !!md.noRefField;
    renderTray();
    renderElementRow();
  }

  // ElevenLabs voices come from the user's own account, read once per page (again after a failed read).
  async function loadVoices() {
    if (state.voices === 'loading' || Array.isArray(state.voices)) return;
    els.voice.textContent = '';
    const tts = state.models.find((m) => m.provider === 'elevenlabs');
    if (!tts || !tts.ready) {
      els.voice.append(opt('', 'Add an ElevenLabs key in Settings'));
      els.voice.disabled = true;
      return;
    }
    state.voices = 'loading';
    els.voice.append(opt('', 'Loading voices...'));
    els.voice.disabled = true;
    let r = null;
    try { r = await call('/api/voices'); } catch (_) { r = null; }
    const list = r && r.ok && r.data && Array.isArray(r.data.voices) ? r.data.voices : null;
    state.voices = list && list.length ? list : null;
    els.voice.textContent = '';
    if (!state.voices) {
      const missing = r && r.data && r.data.error === 'key_missing';
      els.voice.append(opt('', missing ? 'Add an ElevenLabs key in Settings' : 'Could not load voices'));
      return;
    }
    els.voice.disabled = false;
    for (const v of state.voices) els.voice.append(opt(v.id, v.category ? `${v.name} · ${v.category}` : v.name));
  }

  function renderPresets() {
    const prevCam = els.camera.value;
    const prevStyle = els.style.value;
    els.camera.textContent = '';
    els.camera.append(opt('', 'None'));
    const groups = new Map();
    for (const p of state.presets.camera) {
      const g = p.group || 'Other';
      if (!groups.has(g)) groups.set(g, []);
      groups.get(g).push(p);
    }
    for (const [g, items] of groups) {
      const og = el('optgroup', { label: g });
      items.forEach((p) => og.append(opt(p.id, p.name)));
      els.camera.append(og);
    }
    els.style.textContent = '';
    els.style.append(opt('', 'None'));
    state.presets.styles.forEach((s) => els.style.append(opt(s.id, s.name)));
    els.camera.value = prevCam;
    els.style.value = prevStyle;
  }

  // Links from the homepage menus land on #mode or #mode/model-id (e.g. #video/fal/kling-3-pro-t2v).
  function readHash() {
    let h = '';
    try { h = decodeURIComponent(location.hash.slice(1)); } catch (_) { h = ''; }
    const i = h.indexOf('/');
    return i < 0 ? { mode: h, model: null } : { mode: h.slice(0, i), model: h.slice(i + 1) };
  }

  function writeHash() {
    if (state.view === 'board') return;
    const m = els.model.value;
    history.replaceState(null, '', `#${state.mode}${m ? '/' + m : ''}`);
  }

  function setMode(id) {
    els.lookAsk.hidden = true;
    if (!MODES[id]) return;
    leaveBoard();
    state.mode = id;
    const md = MODES[id];
    document.querySelectorAll('.mode[data-mode]').forEach((b) =>
      b.setAttribute('aria-pressed', String(b.dataset.mode === id)));
    els.title.textContent = md.title;
    els.sub.textContent = md.sub;
    els.prompt.placeholder = PROMPT_HINT[id] || 'Describe the shot';
    document.title = `${md.title} · Agave Studio`;
    state.refOn = refActive();
    state.refKey = refKeyNow();
    renderModels();
    if (!state.want) writeHash();
  }

  function autosize() {
    const t = els.prompt;
    t.style.height = 'auto';
    t.style.height = Math.min(t.scrollHeight, 180) + 'px';
  }

  function measureDock() {
    document.documentElement.style.setProperty('--dock-h', els.dock.offsetHeight + 'px');
  }

  // ---------- jobs ----------

  // A failed job can be sent again only while its model is still in the studio (models.toml can drop one).
  const canRunAgain = (j) => j.status === 'failed' && !!j.request && !state.retried.has(j.id)
    && state.models.some((m) => m.id === j.request.model);
  const sigOf = (j) => [
    j.status,
    j.error || '',
    typeof j.progress === 'number' ? 'p' : 'n',
    (j.outputs || []).map((o) => o.url).join('|'),
  ].join('::');

  function outputNode(out, job) {
    const alt = (job.prompt || 'Generated output').slice(0, 140);
    if (out.kind === 'video') {
      const v = el('video', { src: out.url, autoplay: true, loop: true, playsinline: true, muted: true, preload: 'auto', 'aria-label': alt });
      v.muted = true;
      return v;
    }
    if (out.kind === 'audio') return el('audio', { src: out.url, controls: true, preload: 'metadata' });
    if (out.kind === 'model') {
      loadViewer();
      const poster = (job.outputs || []).find((o) => o.kind === 'image');
      return el('model-viewer', {
        src: out.url, poster: poster ? poster.url : '', alt, 'camera-controls': '', 'auto-rotate': '',
        'shadow-intensity': '1', 'interaction-prompt': 'none', loading: 'lazy',
      });
    }
    return el('img', { src: out.url, alt, loading: 'lazy', decoding: 'async' });
  }

  // The 3D viewer (Google's model-viewer, vendored in /vendor) loads the first time a model card shows.
  let viewerLoading = null;
  function loadViewer() {
    if (!viewerLoading) viewerLoading = import('/vendor/model-viewer.min.js').catch(() => { viewerLoading = null; });
    return viewerLoading;
  }

  // One link per file: GLB for the web, USDZ for Apple AR, STL and 3MF for slicers.
  function modelLinks(job) {
    const files = (job.outputs || []).filter((o) => o.kind === 'model');
    return el('span', { class: 'model-links' }, files.map((o) => {
      const ext = (o.url.split('.').pop() || '').toUpperCase();
      return el('a', { href: o.url, download: '', title: `Download ${ext}` }, [ext]);
    }));
  }

  function buildCard(job, isNew) {
    const first = (job.outputs || []).find((o) => o && o.url);
    const label = STATUS_LABEL[job.status] || job.status;
    let fill = null;
    let media;
    if (job.status === 'done' && first) {
      media = el('div', { class: first.kind === 'audio' ? 'media audio-only' : 'media' }, [outputNode(first, job)]);
    } else if (job.status === 'done') {
      media = el('div', { class: 'media' }, [el('div', { class: 'media-msg' }, ['Finished with no output.'])]);
    } else if (job.status === 'failed') {
      media = el('div', { class: 'media' }, [el('div', { class: 'media-msg' }, ['Failed'])]);
    } else {
      fill = el('i');
      paint(fill, job.progress);
      const bar = el('div', { class: 'bar' + (typeof job.progress === 'number' ? '' : ' indet') }, [fill]);
      media = el('div', { class: 'media' }, [el('div', { class: 'ph', 'aria-hidden': 'true' }), bar]);
    }
    const link = first && job.status === 'done' && first.kind === 'model'
      ? modelLinks(job)
      : first && job.status === 'done'
      ? el('a', { href: first.url, target: '_blank', rel: 'noopener noreferrer' }, ['Open file'])
      : null;
    const card = el('article', {
      class: `card ${job.status}${isNew ? ' enter' : ''}`,
      'aria-label': `${label}: ${jobModelName(job)}`,
    }, [
      media,
      el('div', { class: 'card-body' }, [
        el('div', { class: 'card-top' }, [
          el('span', { class: 'state' }, [label]),
          el('span', { class: 'card-model', title: job.model || '' }, [jobModelName(job)]),
        ]),
        el('p', { class: 'card-prompt' }, [job.prompt || '']),
        job.status === 'failed' ? el('p', { class: 'card-error', title: job.error || '' }, [jobError(job)]) : null,
        el('div', { class: 'card-foot' }, [el('span', {}, [job.mode || '']), link]),
        cardActions(job, first),
      ]),
    ]);
    return { el: card, fill };
  }

  // Next steps for a finished or failed job: each one feeds the composer or the Edit panel.
  function cardActions(job, first) {
    const btn = (label, title, fn) => {
      const b = el('button', { type: 'button', class: 'act', title }, [label]);
      b.addEventListener('click', (e) => { e.stopPropagation(); fn(b); });
      return b;
    };
    const acts = [];
    const local = first && /^\/outputs\//.test(first.url);
    if (job.status === 'done' && first && first.kind === 'image' && local) {
      acts.push(btn('Animate', 'Make a video from this still', () => animateFrom(first.url)));
      acts.push(btn('Use', 'Add to the reference images', () => addRefs([first.url], 'Added as a reference image.')));
      acts.push(btn('Grid', 'Variations of the same subject in a grid', () => startRecipe(first.url, 'grid4', 'Optional: what changes between frames (default: the pose)')));
      acts.push(btn('Change', 'Change one thing, keep the rest', () => startRecipe(first.url, 'edit', 'What should change? One thing, e.g. a cream oversized tee')));
      acts.push(btn('Save', 'Save as an Element', () => saveAsElement(first.url)));
      acts.push(btn('3D', 'Make a 3D model of this object', () => make3dFrom(first.url)));
      acts.push(btn('Add to edit', 'Add this picture to the Edit timeline as a still', () => addToEdit(first.url, 'image')));
      const r = job.request && job.request.recipe;
      if (r === 'grid4' || r === 'grid9') {
        acts.push(btn('Pick panel', 'Pick one frame of the grid as a reference', (b) => startPick(b.closest('.card'), first.url, r === 'grid4' ? 2 : 3)));
      }
    }
    if (job.status === 'done' && first && first.kind === 'video' && local) {
      acts.push(btn('Continue', 'Start the next clip from the last frame', (b) => continueFrom(first.url, b)));
      acts.push(btn('Add to edit', 'Add this clip to the Edit timeline', () => addToEdit(first.url, 'video')));
    }
    if (canRunAgain(job)) {
      const price = requestsPrice([job.request]);
      acts.push(btn(`Run again · ${price}`, `Send the same request again, on your own key (${priceNote(price)})`, (b) => runAgain(job, b)));
    }
    return acts.length ? el('div', { class: 'card-acts' }, acts) : null;
  }

  function renderGallery() {
    const jobs = [...state.jobs].sort((a, b) => (b.created_at || 0) - (a.created_at || 0));
    const live = new Set();
    jobs.forEach((job, i) => {
      live.add(job.id);
      const sig = sigOf(job);
      let entry = cards.get(job.id);
      if (!entry || entry.sig !== sig) {
        const built = buildCard(job, !entry);
        if (entry) entry.el.replaceWith(built.el);
        else els.gallery.append(built.el);
        entry = { el: built.el, sig, fill: built.fill };
        cards.set(job.id, entry);
      } else {
        paint(entry.fill, job.progress);
      }
      // CSS order keeps newest first without moving nodes (moving a playing video pauses it).
      entry.el.style.order = String(i);
    });
    for (const [id, entry] of cards) {
      if (!live.has(id)) {
        entry.el.remove();
        cards.delete(id);
      }
    }
    const active = jobs.filter(isActive).length;
    const again = jobs.filter(canRunAgain);
    els.retryAll.hidden = !again.length;
    if (again.length) {
      const price = requestsPrice(again.map((j) => j.request));
      els.retryAll.textContent = `Run ${again.length} failed again · ${price}`;
      els.retryAll.title = `Send ${again.length === 1 ? 'that request' : `those ${again.length} requests`} again, on your own key (${priceNote(price)})`;
    }
    els.empty.hidden = jobs.length > 0;
    els.gallery.hidden = jobs.length === 0;
    els.count.textContent = jobs.length
      ? `${jobs.length} ${jobs.length === 1 ? 'job' : 'jobs'}${active ? ` · ${active} active` : ''}`
      : '';
    if (boardMod) boardMod.jobsChanged();
  }

  function upsertJob(job) {
    state.jobs = [job, ...state.jobs.filter((j) => j.id !== job.id)];
    renderGallery();
  }

  function scheduleNext() {
    clearTimeout(timer);
    if (state.jobs.some(isActive)) timer = setTimeout(refreshJobs, POLL_MS);
    else if (state.offline) timer = setTimeout(refreshJobs, RETRY_MS);
  }

  async function refreshJobs() {
    if (refreshing) {
      again = true;
      return;
    }
    clearTimeout(timer);
    refreshing = true;
    try {
      const r = await call('/api/jobs');
      if (!r.ok) throw new Error('jobs ' + r.status);
      setOffline(false);
      const wasActive = state.jobs.filter(isActive).length;
      state.jobs = (r.data && r.data.jobs) || [];
      renderGallery();
      const nowActive = state.jobs.filter(isActive).length;
      // Once nothing is running, the "Queued" note is stale; the result card shows how it went.
      if (wasActive && !nowActive && /^(Queued|Joining)/.test(els.status.textContent)) setStatus('');
      if (!state.staticLoaded) await loadStatic();
    } catch (_) {
      setOffline(true);
    } finally {
      refreshing = false;
    }
    if (again) {
      again = false;
      return refreshJobs();
    }
    scheduleNext();
  }

  async function loadStatic() {
    try {
      const [m, p, k] = await Promise.all([call('/api/models'), call('/api/presets'), call('/api/keys')]);
      if (m.ok) state.models = (m.data && m.data.models) || [];
      if (p.ok) {
        state.presets = { camera: (p.data && p.data.camera) || [], styles: (p.data && p.data.styles) || [] };
        renderPresets();
      }
      if (k.ok) state.keys = (k.data && k.data.keys) || [];
      state.staticLoaded = m.ok && p.ok;
      renderModels();
      if (state.staticLoaded) writeHash();
      // Cards drawn before the catalog loaded show raw ids (fal/flux-schnell); redraw them with model names.
      if (m.ok) { for (const e of cards.values()) e.sig = ''; renderGallery(); }
      updateEmpty();
    } catch (_) {
      setOffline(true);
    }
  }

  function updateEmpty() {
    els.emptyKey.hidden = !(state.staticLoaded && state.keys.length && !state.keys.some((k) => k.set && !k.reads_only));
  }

  // ---------- submit ----------

  async function submit(e) {
    e.preventDefault();
    if (state.busy) return;
    const md = currentMode();
    const prompt = els.prompt.value.trim();
    // Grids and sheets have a built-in prompt; "Change one thing" needs to be told the change.
    const chosen = !els.recipeWrap.hidden ? els.recipe.value : '';
    if (!prompt && !md.optionalPrompt && (!chosen || chosen === 'edit')) {
      setStatus(chosen === 'edit' ? 'Say the one thing to change.' : 'Write a prompt first.', 'warn');
      els.prompt.focus();
      return;
    }
    const model = els.model.value;
    if (!model) {
      setStatus(state.staticLoaded ? 'No model for this mode.' : 'Models are still loading.', 'warn');
      return;
    }
    if (md.needsRef && !refActive()) {
      setStatus(md.modality === '3d' ? 'Add 1 to 4 pictures of the object first (front, side, back), or press 3D on a picture you made.' : `${md.title} needs a reference image.`, 'warn');
      els.ref.focus();
      return;
    }
    const url = els.ref.value.trim();
    if (!md.noRefField && url && !/^https:\/\/\S+$/i.test(url)) {
      setStatus('Reference URL must start with https://.', 'warn');
      els.ref.focus();
      return;
    }
    const seedText = els.seed.value.trim();
    const seed = seedText === '' ? null : Number(seedText);
    if (seed !== null && (!Number.isInteger(seed) || seed < 0)) {
      setStatus('Seed must be a whole number, 0 or more.', 'warn');
      els.seed.focus();
      return;
    }
    const recipe = !els.recipeWrap.hidden && els.recipe.value ? els.recipe.value : null;
    if (recipe && recipe !== 'sheet' && !refActive()) {
      setStatus('This recipe works from a reference image. Add one first.', 'warn');
      return;
    }
    const m = selectedModel();
    const own = refActive() ? ownRefs() : [];
    const multi = m && (m.max_refs || 0) > 0;
    if (!multi && own.length > 1) {
      setStatus('This model takes one image. Remove some, or pick a model that takes several.', 'warn');
      return;
    }

    const body = {
      model,
      mode: effectiveMode(),
      prompt,
      camera: md.camera && els.camera.value ? els.camera.value : null,
      style: md.style && els.style.value ? els.style.value : null,
      aspect: state.chips.aspect,
      duration: state.chips.duration,
      resolution: state.chips.resolution,
      audio: state.chips.audio,
      seed,
      image_url: multi ? null : (own[0] || null),
    };
    if (multi && own.length) body.image_urls = own;
    if (md.modality === 'audio') {
      if (!els.voice.value) {
        setStatus('Pick a voice first. Voices come from your ElevenLabs account.', 'warn');
        return;
      }
      body.voice_id = els.voice.value;
    }
    if (recipe) body.recipe = recipe;
    // A look on image to video goes on the start picture first (the picture sets the video's look).
    if (body.mode === 'i2v' && body.style && body.image_url && !state.lookOk.has(`${body.image_url}|${body.style}`)) {
      askLook(body.image_url, body.style);
      return;
    }
    els.lookAsk.hidden = true;

    const count = Number(els.countSel.value) || 1;
    state.busy = true;
    els.generate.disabled = true;
    setStatus(count > 1 ? `Sending ${count}...` : 'Sending...');
    try {
      let sent = 0;
      for (let i = 0; i < count; i++) {
        const b = { ...body, seed: seed === null ? null : seed + i };
        const ok = await sendGenerate(b, count > 1 ? ` (${i + 1} of ${count})` : '');
        if (!ok) break;
        sent++;
      }
      if (sent) refreshJobs();
    } finally {
      state.busy = false;
      els.generate.disabled = false;
    }
  }

  // ---------- looks on image to video ----------
  // The start picture fixes a video's look, so look words alone barely show. Agave offers to put the look on the
  // picture first with an image edit (its list price on the button, sent only on the click), then you animate it.
  function lookEditModel() {
    const ok = state.models.filter((m) => m.modality === 'image' && (m.modes || []).includes('i2i') && m.price);
    return ok.find((m) => m.id === 'fal/seedream-4-edit' && m.ready) || ok.find((m) => m.ready) || ok.find((m) => m.id === 'fal/seedream-4-edit') || null;
  }
  const lookName = (id) => ((state.presets.styles || []).find((s) => s.id === id) || { name: id }).name;

  function askLook(pic, style) {
    const m = lookEditModel();
    if (!m) {
      state.lookOk.add(`${pic}|${style}`);
      setStatus('No image edit model is set up to put the look on the picture, so the look is sent as words only.', 'warn');
      return;
    }
    const cents = Math.max(1, Math.ceil(Math.round(m.price.usd * 1e6) / 1e4));
    els.lookAskText.textContent = `The start picture sets the video's look. Put ${lookName(style)} on the picture first with ${m.name}, then animate it.`;
    els.lookApply.textContent = `Apply ${lookName(style)} · ${cents}¢`;
    els.lookApply.title = `About ${cents}¢ on your own key (${m.vendor}'s list price)`;
    els.lookApply.onclick = () => applyLook(pic, style, m);
    els.lookSkip.onclick = () => {
      els.lookAsk.hidden = true;
      state.lookOk.add(`${pic}|${style}`);
      els.form.requestSubmit();
    };
    els.lookAsk.hidden = false;
    setStatus('');
  }

  async function applyLook(pic, style, m) {
    els.lookAsk.hidden = true;
    state.busy = true;
    els.generate.disabled = true;
    setStatus(`Putting ${lookName(style)} on the picture with ${m.name}...`);
    try {
      // No aspect: the engine keeps the picture's own frame for a look.
      const look = { model: m.id, mode: 'i2i', recipe: 'look', prompt: '', style, image_url: pic };
      const r = await call('/api/generate', { method: 'POST', body: { ...look, estimate: spendNote(look) } });
      if (!(r.status === 202 && r.data && r.data.job)) {
        const err = r.data && r.data.error;
        if (r.status === 412 && err === 'missing_key') {
          setStatus(`Add ${r.data.key} in Settings to put the look on the picture.`, 'warn');
          openSettings(r.data.key);
        } else {
          setStatus(ERRORS[err] || `The look edit was refused (${err || r.status}).`, 'err');
        }
        return;
      }
      upsertJob(r.data.job);
      refreshJobs();
      let job = r.data.job;
      for (let i = 0; i < 150 && isActive(job); i++) {
        await new Promise((ok) => setTimeout(ok, 2000));
        const j = await call(`/api/jobs/${encodeURIComponent(job.id)}`);
        if (j.ok && j.data && j.data.job) job = j.data.job;
      }
      const out = job.status === 'done' && job.outputs && job.outputs[0] && job.outputs[0].url;
      if (!out) {
        setStatus('The look edit did not finish, so the picture is unchanged. Its card says why.', 'err');
        return;
      }
      // Swap only if that picture is still the start picture; otherwise the edit stays in the gallery.
      const own = ownRefs();
      if (own[0] !== pic || effectiveMode() !== 'i2v') {
        setStatus(`${lookName(style)} is on a copy of the picture, in the gallery. The composer changed meanwhile, so nothing was swapped.`);
        return;
      }
      if (els.ref.value.trim() === pic) els.ref.value = '';
      state.refs = [out, ...state.refs.filter((u) => u !== pic)];
      renderTray();
      syncRefMode();
      state.lookOk.add(`${out}|${style}`);
      setStatus(`${lookName(style)} is on the picture. Press Generate to animate it.`);
      refreshJobs();
    } catch (_) {
      setOffline(true);
      setStatus('Could not reach the engine.', 'err');
    } finally {
      state.busy = false;
      els.generate.disabled = false;
    }
  }

  // Sends one generate request and reports the outcome in the status line. True when queued.
  async function sendGenerate(body, label = '') {
    try {
      // A request sent again (Run again) carries its old note; the one sent is always this request's own.
      const { estimate: _old, ...req } = body;
      const r = await call('/api/generate', { method: 'POST', body: { ...req, estimate: spendNote(req) } });
      if (r.status === 202 && r.data && r.data.job) {
        upsertJob(r.data.job);
        setStatus('Queued.' + label);
        return true;
      }
      const err = r.data && r.data.error;
      if (r.status === 412 && err === 'missing_key') {
        setStatus(`Add ${r.data.key} in Settings to run this model.`, 'warn');
        openSettings(r.data.key);
      } else if (r.status === 409 && err === 'still_first') {
        setStatus('Picture before video: make a still first, then press Animate on it. (You can turn this off in Settings.)', 'warn');
      } else if (err === 'too_many_refs') {
        setStatus(`This model takes up to ${r.data.max} reference images; this request has ${r.data.got}.`, 'warn');
      } else if (err === 'too_many_element_tags') {
        setStatus(`This model takes up to ${r.data.max} @elements next to its start picture; this prompt has ${r.data.got}.`, 'warn');
      } else if (err === 'refs_need_image_mode') {
        setStatus('An @element brings its pictures along, so pick a model that takes reference images.', 'warn');
      } else {
        setStatus(ERRORS[err] || (err ? `The engine refused this request (${err}).` : `Request failed (${r.status}).`), 'err');
      }
    } catch (_) {
      setOffline(true);
      setStatus('Could not reach the engine.', 'err');
    }
    return false;
  }

  // A provider's refusal in plain words: what happened and what to do. Its own text stays in the tooltip.
  function jobError(job) {
    const e = String(job.error || '');
    const code = (e.match(/provider HTTP (\d{3})/) || [])[1];
    const m = job.request && state.models.find((x) => x.id === job.request.model);
    const key = (m && m.key) || 'your key';
    const credit = /balance|credit|billing|payment/i.test(e);
    if (code === '402' || ((code === '401' || code === '403') && credit)) {
      return `The provider says this account is out of credit or needs billing set up (HTTP ${code}). Add credit with the provider, then run it again.`;
    }
    if (code === '401' || code === '403') return `The provider refused ${key} (HTTP ${code}). Check the key in Settings, then run it again.`;
    if (code === '429') return `The provider is limiting requests on ${key} (HTTP 429: too many, or a quota is used up). Wait, then run it again.`;
    return e || 'Generation failed.';
  }

  async function runAgain(job, b) {
    if (b) b.disabled = true;
    state.retried.add(job.id);
    const ok = await sendGenerate(job.request, ' Running it again.');
    if (!ok) state.retried.delete(job.id);
    for (const e of cards.values()) e.sig = '';
    renderGallery();
    if (ok) refreshJobs();
  }

  async function runFailedAgain() {
    const failed = state.jobs.filter(canRunAgain);
    els.retryAll.disabled = true;
    let n = 0;
    for (const j of failed) {
      state.retried.add(j.id);
      if (await sendGenerate(j.request)) n++;
      else state.retried.delete(j.id);
    }
    els.retryAll.disabled = false;
    for (const e of cards.values()) e.sig = '';
    renderGallery();
    if (n) {
      setStatus(`Ran ${n} failed job${n === 1 ? '' : 's'} again.`);
      refreshJobs();
    }
  }

  // ---------- references, uploads ----------

  const MAX_TRAY = 4;
  const MAX_UPLOAD = 40 * 1024 * 1024;

  function fileToDataUrl(file) {
    return new Promise((resolve, reject) => {
      const r = new FileReader();
      r.onload = () => resolve(r.result);
      r.onerror = () => reject(new Error('read failed'));
      r.readAsDataURL(file);
    });
  }

  // Saves a data: URL on the engine and returns its /outputs/... address.
  async function uploadData(data) {
    const r = await call('/api/uploads', { method: 'POST', body: { data } });
    if (r.status === 201 && r.data && r.data.url) return r.data;
    const code = (r.data && r.data.error) || `upload failed (${r.status})`;
    throw new Error(code === 'unsupported_file' ? 'That file type is not supported.' : code === 'file_too_large' ? 'That file is too large (40 MB max).' : code);
  }

  async function addFiles(files) {
    const list = [...files].filter((f) => /^image\/(png|jpeg|webp)$/.test(f.type));
    if (!list.length) {
      setStatus('Add PNG, JPEG or WebP images.', 'warn');
      return;
    }
    setStatus(`Uploading ${list.length} image${list.length === 1 ? '' : 's'}...`);
    const urls = [];
    try {
      for (const f of list.slice(0, MAX_TRAY)) {
        if (f.size > MAX_UPLOAD) throw new Error('That file is too large (40 MB max).');
        urls.push((await uploadData(await fileToDataUrl(f))).url);
      }
      addRefs(urls, `Added ${urls.length} reference image${urls.length === 1 ? '' : 's'}.`);
    } catch (e) {
      if (urls.length) addRefs(urls);
      setStatus(e.message || 'Upload failed.', 'err');
    }
  }

  function addRefs(urls, msg) {
    for (const u of urls) {
      if (state.refs.includes(u)) continue;
      state.refs.push(u);
      if (state.refs.length > MAX_TRAY) state.refs.shift();
    }
    renderTray();
    syncRefMode();
    // A grid or a one-thing change keeps the first picture's shape.
    if (['grid4', 'grid9', 'edit'].includes(els.recipe.value) && state.refs.length) matchAspect(state.refs[0]);
    if (msg) setStatus(msg);
  }

  function renderTray() {
    els.lookAsk.hidden = true;
    els.tray.textContent = '';
    els.tray.hidden = !state.refs.length || !!currentMode().noRefField;
    state.refs.forEach((u, i) => {
      const x = el('button', { type: 'button', class: 'tray-x', 'aria-label': `Remove reference ${i + 1}` }, ['×']);
      x.addEventListener('click', () => {
        state.refs = state.refs.filter((r) => r !== u);
        renderTray();
        syncRefMode();
      });
      els.tray.append(el('div', { class: 'tray-item' }, [el('img', { src: u, alt: `Reference ${i + 1}` }), x]));
    });
    if (els.library.classList.contains('open')) renderNewElementImages();
  }

  function renderElementRow() {
    els.elRow.textContent = '';
    const show = state.elements.length && !currentMode().noRefField;
    els.elRow.hidden = !show;
    if (!show) return;
    els.elRow.append(el('span', { class: 'k' }, ['Elements']));
    for (const e of state.elements.slice(0, 16)) {
      const b = el('button', { type: 'button', class: 'el-chip', title: e.name }, [`@${e.tag}`]);
      b.addEventListener('click', () => insertTag(e.tag));
      els.elRow.append(b);
    }
  }

  function insertTag(tag) {
    const t = els.prompt;
    const at = t.selectionStart ?? t.value.length;
    const before = t.value.slice(0, at);
    const pad = before && !/\s$/.test(before) ? ' ' : '';
    t.value = `${before}${pad}@${tag} ${t.value.slice(at)}`;
    const pos = before.length + pad.length + tag.length + 2;
    t.focus();
    t.setSelectionRange(pos, pos);
    autosize();
    syncRefMode();
  }

  // The best ready image model that takes references (several first).
  function pickEditModel() {
    const ok = state.models.filter((m) => m.modality === 'image' && (m.modes || []).includes('i2i'));
    return ok.find((m) => m.ready && m.max_refs > 0) || ok.find((m) => m.ready) || ok.find((m) => m.max_refs > 0) || ok[0] || null;
  }

  function useModel(m) {
    if (!m || ![...els.model.options].some((o) => o.value === m.id)) return;
    els.model.value = m.id;
    renderChips();
    renderCost();
    writeHash();
  }

  function startRecipe(url, recipe, hint) {
    state.refs = [url];
    els.ref.value = '';
    setMode('image');
    els.recipe.value = recipe;
    useModel(pickEditModel());
    renderTray();
    syncRefMode();
    els.prompt.value = '';
    els.prompt.placeholder = hint;
    autosize();
    els.prompt.focus();
    setStatus(recipe === 'edit' ? 'Say the one thing to change, then Generate. Everything else stays.' : 'Press Generate for the grid, or say what should change between frames.');
    matchAspect(url);
  }

  // Sets the Aspect chip to the model's closest shape to the picture, so a grid or a change keeps its framing.
  function matchAspect(url) {
    const img = new Image();
    img.onload = () => {
      const m = selectedModel();
      const list = (m && m.params && m.params.aspect) || [];
      if (!img.naturalWidth || !img.naturalHeight || !list.length) return;
      const want = Math.log(img.naturalWidth / img.naturalHeight);
      const diff = (a) => {
        const [w, h] = String(a).split(':').map(Number);
        return w > 0 && h > 0 ? Math.abs(Math.log(w / h) - want) : Infinity;
      };
      const best = list.reduce((x, y) => (diff(y) < diff(x) ? y : x));
      const sel = $('#aspect');
      sel.value = String(best);
      state.chips.aspect = String(best);
    };
    img.src = url;
  }

  function make3dFrom(url) {
    state.refs = [url];
    els.ref.value = '';
    setMode('3d');
    renderTray();
    syncRefMode();
    els.prompt.value = '';
    autosize();
    setStatus('Add up to 3 more views of the same object (side, back) for a better model, then Generate.');
  }

  function animateFrom(url) {
    state.refs = [url];
    els.ref.value = '';
    setMode('video');
    renderTray();
    syncRefMode();
    els.prompt.placeholder = 'Describe the motion and the camera, e.g. slow push in, she turns to the window';
    els.prompt.focus();
    setStatus('This still starts the video. Describe the motion, then Generate.');
  }

  function waitFor(target, ev) {
    return new Promise((resolve, reject) => {
      target.addEventListener(ev, resolve, { once: true });
      target.addEventListener('error', () => reject(new Error('media failed to load')), { once: true });
    });
  }

  // Grabs the last frame of a clip (served from the same origin, so the canvas is readable).
  async function lastFrame(url) {
    const v = document.createElement('video');
    v.muted = true;
    v.preload = 'auto';
    v.src = url;
    await waitFor(v, 'loadedmetadata');
    v.currentTime = Math.max(0, v.duration - 0.05);
    await waitFor(v, 'seeked');
    const c = document.createElement('canvas');
    c.width = v.videoWidth;
    c.height = v.videoHeight;
    c.getContext('2d').drawImage(v, 0, 0);
    return c.toDataURL('image/png');
  }

  async function continueFrom(url, b) {
    if (b) b.disabled = true;
    setStatus('Taking the last frame...');
    try {
      const up = await uploadData(await lastFrame(url));
      animateFrom(up.url);
      setStatus('The last frame starts the next clip. Describe what happens next, then Generate.');
    } catch (e) {
      setStatus(e.message || 'Could not take the last frame.', 'err');
    } finally {
      if (b) b.disabled = false;
    }
  }

  // Lays a clickable n x n grid over a grid image; the chosen cell is cropped and added as a reference.
  function startPick(card, url, n) {
    const media = card && card.querySelector('.media');
    const img = media && media.querySelector('img');
    if (!img) return;
    const old = media.querySelector('.picker');
    if (old) { old.remove(); return; }
    const box = media.getBoundingClientRect();
    const scale = Math.min(box.width / img.naturalWidth, box.height / img.naturalHeight);
    const w = img.naturalWidth * scale;
    const h = img.naturalHeight * scale;
    const picker = el('div', { class: 'picker', role: 'group', 'aria-label': 'Pick a panel' });
    Object.assign(picker.style, {
      width: `${w}px`, height: `${h}px`, left: `${(box.width - w) / 2}px`, top: `${(box.height - h) / 2}px`,
      gridTemplateColumns: `repeat(${n}, 1fr)`,
    });
    for (let i = 0; i < n * n; i++) {
      const cell = el('button', { type: 'button', 'aria-label': `Panel ${i + 1}` }, [String(i + 1)]);
      cell.addEventListener('click', async (e) => {
        e.stopPropagation();
        picker.remove();
        setStatus('Cutting out the panel...');
        try {
          const up = await uploadData(await cropPanel(url, n, i));
          addRefs([up.url], `Panel ${i + 1} added as a reference. Use Change or Animate from here.`);
        } catch (err) {
          setStatus(err.message || 'Could not cut the panel.', 'err');
        }
      });
      picker.append(cell);
    }
    media.append(picker);
    setStatus('Click the panel you want.');
  }

  async function cropPanel(url, n, i) {
    const img = new Image();
    img.src = url;
    await img.decode();
    const w = Math.floor(img.naturalWidth / n);
    const h = Math.floor(img.naturalHeight / n);
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    c.getContext('2d').drawImage(img, (i % n) * w, Math.floor(i / n) * h, w, h, 0, 0, w, h);
    return c.toDataURL('image/png');
  }

  // ---------- elements ----------

  async function loadElements() {
    try {
      const r = await call('/api/elements');
      if (r.ok) state.elements = (r.data && r.data.elements) || [];
    } catch (_) { /* offline: keep what we have */ }
    renderElementRow();
    renderElements();
    syncRefMode();
  }

  function renderNewElementImages() {
    els.elNewImgs.textContent = '';
    const local = state.refs.filter((u) => u.startsWith('/outputs/'));
    local.forEach((u, i) => els.elNewImgs.append(el('img', { src: u, alt: `Image ${i + 1}` })));
    $('#elNewHint').hidden = local.length > 0;
  }

  function renderElements() {
    els.elList.textContent = '';
    if (!state.elements.length) {
      els.elList.append(el('p', { class: 'muted' }, ['No elements yet.']));
      return;
    }
    for (const e of state.elements) {
      const ins = el('button', { type: 'button', class: 'small-btn' }, ['Insert']);
      ins.addEventListener('click', () => { closePanel(); insertTag(e.tag); });
      const del = el('button', { type: 'button', class: 'link-btn' }, ['Delete']);
      del.addEventListener('click', async () => {
        const r = await call('/api/elements/delete', { method: 'POST', body: { tag: e.tag } });
        if (r.ok) await loadElements();
      });
      els.elList.append(el('div', { class: 'el-item' }, [
        el('img', { src: e.images[0], alt: '' }),
        el('div', { class: 'el-text' }, [
          el('span', { class: 'el-tag' }, [`@${e.tag}`]),
          el('span', { class: 'el-name' }, [e.name]),
          el('span', { class: 'el-kind' }, [`${e.kind} · ${e.images.length} image${e.images.length === 1 ? '' : 's'}`]),
        ]),
        el('div', { class: 'el-btns' }, [ins, del]),
      ]));
    }
  }

  function saveAsElement(url) {
    addRefs([url]);
    openPanel(els.library, () => {
      renderNewElementImages();
      els.elTag.focus();
    });
  }

  async function saveElement(ev) {
    ev.preventDefault();
    const images = state.refs.filter((u) => u.startsWith('/outputs/'));
    const tag = els.elTag.value.trim().toLowerCase().replace(/^@/, '');
    const name = els.elName.value.trim();
    if (!images.length) { els.elNote.textContent = 'Add reference images in the composer first.'; return; }
    if (!/^[a-z0-9][a-z0-9_-]{0,31}$/.test(tag)) { els.elNote.textContent = 'Tag: letters, numbers, - or _, starting with a letter or number.'; return; }
    if (!name) { els.elNote.textContent = 'Say who or what this is, in a few words.'; els.elName.focus(); return; }
    const r = await call('/api/elements', { method: 'POST', body: { tag, name, kind: els.elKind.value, images: images.slice(0, 4) } });
    if (r.ok) {
      els.elNote.textContent = `Saved. Type @${tag} in a prompt to use it.`;
      els.elTag.value = '';
      els.elName.value = '';
      await loadElements();
    } else {
      els.elNote.textContent = (r.data && r.data.error) || `Could not save (${r.status}).`;
    }
  }

  // ---------- Agave Edit (timeline) ----------
  // The timeline is a list of pieces: a trimmed part of a video, or a picture shown for a set time.
  // It plays in the preview here; Export renders the same list on the engine (/api/assemble).

  const FRAMES = [['original', 'Original'], ['16:9', '16:9'], ['9:16', '9:16'], ['1:1', '1:1'], ['4:5', '4:5']];
  const FRAME_SIZE = { '16:9': [1920, 1080], '9:16': [1080, 1920], '1:1': [1080, 1080], '4:5': [1080, 1350] };
  const STILL_DEFAULT = 3;
  const STILL_MAX = 60;
  const MIN_PIECE = 0.1;
  const MAX_EDIT_CLIPS = 200;
  const MAX_CAPTION = 120;
  const pv = { playing: false, loading: false, last: 0, song: null, measured: new Map() };

  const num = (v) => (typeof v === 'number' && Number.isFinite(v) && v >= 0 ? v : null);
  const kindOf = (url) => (/\.(png|jpe?g|webp)$/i.test(url) ? 'image' : 'video');
  const fmtTime = (t) => `${Math.floor(t / 60)}:${(t % 60).toFixed(1).padStart(4, '0')}`;

  function normClip(c) {
    if (!c || typeof c.url !== 'string' || !c.url.startsWith('/outputs/')) return null;
    const kind = c.kind === 'image' || c.kind === 'video' ? c.kind : kindOf(c.url);
    return {
      url: c.url,
      kind,
      from: kind === 'image' ? 0 : (num(c.from) ?? 0),
      to: num(c.to),
      len: num(c.len),
      caption: typeof c.caption === 'string' ? c.caption.slice(0, MAX_CAPTION) : '',
    };
  }

  // Seconds a piece lasts on the timeline.
  function pieceLen(c) {
    if (c.kind === 'image') return Math.min(STILL_MAX, Math.max(MIN_PIECE, c.to ?? STILL_DEFAULT));
    const end = Math.min(c.to ?? c.len ?? 0, c.len ?? Infinity);
    return Math.max(0, end - c.from);
  }

  const editTotal = () => state.edit.reduce((s, c) => s + pieceLen(c), 0);

  // Which piece is under time t, where it starts, and how far into it t is.
  function locate(t) {
    let start = 0;
    for (let i = 0; i < state.edit.length; i++) {
      const d = pieceLen(state.edit[i]);
      if (t < start + d || i === state.edit.length - 1) return { i, start, local: Math.min(Math.max(0, t - start), d), d };
      start += d;
    }
    return null;
  }

  function editDoc() {
    return {
      clips: state.edit.map(({ url, kind, from, to, len, caption }) => ({ url, kind, from, to, len, caption })),
      frame: state.editFrame,
      fill: state.editFill,
      keep_sound: els.keepSound.checked,
      song_url: state.song ? state.song.url : null,
      song_name: state.song ? state.song.name : null,
      auto_captions: state.autoCaps,
      word_fixes: state.wordFixes,
      follow: state.editFollow,
      clean_voice: els.cleanVoice.checked,
      loudness: els.loudness.checked,
    };
  }

  // Timeline sync. The local copy is based on the saved timeline `state.editRev` (clip urls in `state.editBase`).
  // Changes go out 400 ms after the last one; only one save is in flight at a time, and changes made meanwhile go
  // out when it returns. Every save carries the rev it is based on (before the first read, the page reads first).
  // When something else saved first (a pipeline's Add to edit, another tab), the engine answers 409 and the local
  // copy is rebased on the newer timeline: every local change is kept, and clips newly added there are appended.
  // A change that could not be sent is retried with backoff, and kept in localStorage in this tab's own unsent
  // marker (the doc and the clips it was based on), so a reload rebases it instead of losing it. Only the tab that
  // wrote a marker, or the page that took it over at load, removes it. Nothing ever drops a user's edit.
  const UNSENT_PREFIX = 'agave.edit.unsent.';
  const TAB_ID = Math.random().toString(36).slice(2, 10);
  let adopted = []; // marker keys taken over at load (from a closed tab or the last visit): removed once saved
  const SAVE_RETRY_MS = [1000, 2000, 5000, 10000, 20000, 30000];
  let saveTimer = null;
  let saving = false; // a POST is in flight
  let dirty = false; // local changes not yet sent
  let refused = false; // the engine refused the doc as invalid (4xx): no retries until the next change
  let retryTimer = null;
  let retries = 0;
  const clipUrls = (clips) => new Set((clips || []).map((c) => c && c.url));
  const editPending = () => saveTimer !== null || saving || dirty;

  // Undo restores a snapshot of the clips, so it would drop clips added since: it ends when clips arrive from elsewhere.
  function dropUndo() {
    state.undo = null;
    els.undoBtn.hidden = true;
  }

  // The local copy, and whether it holds changes the engine has not saved yet (with the clips they were based on).
  function writeLocal(unsent) {
    const doc = editDoc();
    try {
      if (unsent === false) {
        // Saved: this tab's marker and the ones it took over go first, so a full storage cannot leave them behind.
        for (const k of [UNSENT_PREFIX + TAB_ID, ...adopted]) localStorage.removeItem(k);
        adopted = [];
      }
      if (unsent) localStorage.setItem(UNSENT_PREFIX + TAB_ID, JSON.stringify({ tab: TAB_ID, t: Date.now(), rev: state.editRev, base: [...state.editBase], doc }));
      localStorage.setItem('agave.edit', JSON.stringify(doc));
    } catch (_) { /* private mode or full */ }
  }

  // Unsent markers left in this browser (this tab's from before a reload, or another tab's), newest first.
  function readMarkers() {
    const out = [];
    try {
      for (let i = 0; i < localStorage.length; i++) {
        const k = localStorage.key(i);
        if (!k || !k.startsWith(UNSENT_PREFIX)) continue;
        try {
          const m = JSON.parse(localStorage.getItem(k));
          if (m && m.doc && Array.isArray(m.doc.clips) && Array.isArray(m.base)) out.push({ ...m, key: k });
        } catch (_) { /* a broken marker is skipped */ }
      }
    } catch (_) { /* private mode */ }
    return out.sort((a, b) => (b.t || 0) - (a.t || 0));
  }

  function saveEdit() {
    // Undo only covers Remove pauses until the timeline changes again.
    if (state.undo && JSON.stringify(state.edit) !== state.undoAfter) dropUndo();
    dirty = true;
    refused = false;
    writeLocal(true);
    clearTimeout(saveTimer);
    saveTimer = setTimeout(() => { saveTimer = null; flushEdit(); }, 400);
  }

  function retryLater() {
    clearTimeout(retryTimer);
    const ms = SAVE_RETRY_MS[Math.min(retries, SAVE_RETRY_MS.length - 1)];
    retries++;
    retryTimer = setTimeout(() => { retryTimer = null; flushEdit(); }, ms);
  }

  async function flushEdit() {
    if (saving || refused || !dirty) return; // the save in flight sends the rest when it returns
    clearTimeout(retryTimer);
    retryTimer = null;
    if (state.editRev === null) {
      // No read has come back yet: read first (it rebases this change onto the saved timeline and sends it).
      await loadEditServer();
      if (state.editRev === null && retryTimer === null) retryLater();
      return;
    }
    // The doc and the rev it is based on are taken together, at send time.
    const doc = editDoc();
    const rev = state.editRev;
    dirty = false;
    saving = true;
    let r = null;
    try { r = await call('/api/edit', { method: 'POST', body: { ...doc, rev } }); } catch (_) { r = null; }
    saving = false;
    if (r && r.ok && r.data && typeof r.data.rev === 'number') {
      retries = 0;
      state.editRev = r.data.rev;
      state.editBase = clipUrls(doc.clips);
      writeLocal(dirty ? true : false);
    } else if (r && r.status === 409 && r.data && r.data.error === 'edit_changed' && r.data.edit) {
      rebaseEdit(r.data.edit, r.data.rev);
      dirty = true;
      writeLocal(true);
    } else if (r && r.status >= 400 && r.status < 500 && r.status !== 408 && r.status !== 429) {
      // The engine refused this timeline as it is: retrying the same doc cannot help. It stays here, marked unsent.
      dirty = true;
      refused = true;
      const why = r.data && typeof r.data.error === 'string' ? ` (${r.data.error})` : '';
      els.editNote.textContent = `The engine could not save this timeline${why}. It is kept in this browser; your next change tries again.`;
      return;
    } else {
      // Offline or the engine is busy: try again soon, then less often.
      dirty = true;
      retryLater();
      return;
    }
    if (dirty && saveTimer === null) flushEdit();
  }

  // Keeps the current local timeline (clips, trims, captions, settings) and appends the clips the newer saved
  // timeline has that are new there: in neither the old base nor the local copy. A clip removed here stays removed.
  function rebaseEdit(saved, rev) {
    const local = clipUrls(state.edit);
    const fresh = (Array.isArray(saved.clips) ? saved.clips : [])
      .filter((c) => c && !state.editBase.has(c.url) && !local.has(c.url))
      .map(normClip).filter(Boolean);
    const added = fresh.slice(0, Math.max(0, MAX_EDIT_CLIPS - state.edit.length));
    state.editRev = typeof rev === 'number' ? rev : state.editRev;
    state.editBase = clipUrls(saved.clips);
    if (!fresh.length) return;
    dropUndo();
    state.edit.push(...added);
    if (state.editSel < 0 && state.edit.length) state.editSel = 0;
    writeLocal(true);
    keepCaret(() => { renderEditor(); measureAll(); });
    const n = added.length;
    const cut = fresh.length - n;
    els.editNote.textContent = (n ? `${n} clip${n === 1 ? ' was' : 's were'} added elsewhere (a pipeline or another tab), so ${n === 1 ? 'it is' : 'they are'} now at the end of the timeline.` : '')
      + (cut ? `${n ? ' ' : ''}${cut} clip${cut === 1 ? '' : 's'} added elsewhere did not fit: the timeline holds ${MAX_EDIT_CLIPS}.` : '');
  }

  // Re-renders without taking focus and the caret out of the caption being typed.
  function keepCaret(render) {
    const a = document.activeElement;
    const label = a && a.tagName === 'INPUT' && els.inspector.contains(a) ? a.getAttribute('aria-label') : null;
    const sel = label ? [a.selectionStart, a.selectionEnd] : null;
    render();
    if (!label) return;
    const again = [...els.inspector.querySelectorAll('input')].find((x) => x.getAttribute('aria-label') === label);
    if (!again) return;
    again.focus({ preventScroll: true });
    try { again.setSelectionRange(sel[0], sel[1]); } catch (_) { /* number inputs have no caret */ }
  }

  function applyEditDoc(v) {
    // Older saves were a bare list of { url, caption }.
    const doc = Array.isArray(v) ? { clips: v } : (v || {});
    state.edit = (Array.isArray(doc.clips) ? doc.clips : []).map(normClip).filter(Boolean).slice(0, MAX_EDIT_CLIPS);
    state.editFrame = FRAMES.some((f) => f[0] === doc.frame) ? doc.frame : 'original';
    state.editFill = doc.fill === true;
    state.editFollow = doc.follow === true;
    els.cleanVoice.checked = doc.clean_voice === true;
    els.loudness.checked = doc.loudness === true;
    if (typeof doc.keep_sound === 'boolean') els.keepSound.checked = doc.keep_sound;
    state.song = typeof doc.song_url === 'string' && doc.song_url.startsWith('/outputs/') ? { url: doc.song_url, name: doc.song_name || 'Soundtrack' } : null;
    state.autoCaps = CAP_STYLES.some((c) => c[0] === doc.auto_captions) ? doc.auto_captions : 'off';
    state.wordFixes = (Array.isArray(doc.word_fixes) ? doc.word_fixes : [])
      .filter((f) => f && typeof f.from === 'string' && f.from.trim())
      .map((f) => ({ from: f.from.trim().slice(0, 40), to: typeof f.to === 'string' ? f.to.trim().slice(0, 40) : '' }))
      .slice(0, 50);
    state.editSel = state.edit.length ? 0 : -1;
    state.ph = 0;
  }

  // The local copy from the last visit. With an unsent marker it holds changes the engine never saved: they are
  // rebased onto the saved timeline at the first read (on the clips they were based on). Without one, the copy was
  // the saved timeline, so its own clips are the base.
  function loadEdit() {
    const marks = readMarkers();
    if (!marks.length) {
      try { applyEditDoc(JSON.parse(localStorage.getItem('agave.edit') || '[]')); } catch (_) { applyEditDoc([]); }
      state.editBase = clipUrls(state.edit);
      return;
    }
    // The newest unsent doc becomes the local copy; clips the others added (not in their base) are appended.
    applyEditDoc(marks[0].doc);
    state.editBase = new Set(marks[0].base);
    for (const m of marks.slice(1)) {
      const have = clipUrls(state.edit);
      const base = new Set(m.base);
      const extra = m.doc.clips.filter((c) => c && !base.has(c.url) && !have.has(c.url)).map(normClip).filter(Boolean);
      state.edit.push(...extra.slice(0, Math.max(0, MAX_EDIT_CLIPS - state.edit.length)));
    }
    adopted = marks.map((m) => m.key);
    dirty = true;
  }

  // The engine keeps the timeline (data/studio.json), so it survives a restart.
  async function loadEditServer(confirming = false) {
    let read = false;
    try {
      const r = await call('/api/edit');
      if (r.ok && r.data && r.data.edit) {
        read = true;
        const rev = typeof r.data.rev === 'number' ? r.data.rev : null;
        const first = state.editRev === null;
        // The engine's rev only goes up while its data lasts. A lower one is either a late read (an older answer
        // arriving after a newer save) or a reset (data/ was wiped and the count started over): a second read tells
        // them apart, because after a late read it shows the newer rev again.
        const lower = rev !== null && !first && rev < state.editRev;
        if (lower && !confirming) return loadEditServer(true);
        const reset = lower && confirming;
        const newer = rev !== null && (first || rev > state.editRev || reset);
        if (newer && editPending()) {
          if (first || reset || (!saving && saveTimer === null)) {
            // Unsent changes (from before this read, a failed save or the last visit): keep them, add the saved
            // timeline's new clips, and send them on the new rev.
            rebaseEdit(r.data.edit, rev);
            dirty = true; // the rebased copy differs from what any save in flight carries
            if (!refused) flushEdit();
          }
          // Otherwise a save is on its way: it gets a 409 and rebases, so leave the copy and the rev alone.
        } else if (first) {
          // First read: an older local copy wins over an empty saved one.
          if (r.data.edit.clips.length || !state.edit.length) { applyEditDoc(r.data.edit); dropUndo(); writeLocal(false); }
          state.editRev = rev;
          state.editBase = clipUrls(r.data.edit.clips);
        } else if (newer) {
          // A newer saved timeline and nothing local waiting: take it.
          applyEditDoc(r.data.edit);
          dropUndo();
          state.editRev = rev;
          state.editBase = clipUrls(r.data.edit.clips);
          writeLocal(false);
        }
      }
    } catch (_) { /* offline: keep the local copy */ }
    // Unsent changes wait on a read that failed: try again soon instead of waiting for the next change.
    if (!read && dirty && !refused && !saving && saveTimer === null && retryTimer === null) retryLater();
    renderSong();
    keepCaret(renderEditor);
    measureAll();
    if (state.autoCaps !== 'off') loadTranscripts().then(showAutoCap);
    loadFollow();
  }

  // Reads each source's length and picture size once (needed for trims and the Original frame).
  function measure(url, kind) {
    if (pv.measured.has(url)) return pv.measured.get(url);
    const p = new Promise((resolve) => {
      if (kind === 'image') {
        const im = new Image();
        im.onload = () => resolve({ w: im.naturalWidth, h: im.naturalHeight, len: null });
        im.onerror = () => resolve(null);
        im.src = url;
      } else {
        const v = document.createElement('video');
        v.muted = true;
        v.preload = 'metadata';
        v.onloadedmetadata = () => resolve({ w: v.videoWidth, h: v.videoHeight, len: Number.isFinite(v.duration) ? v.duration : null });
        v.onerror = () => resolve(null);
        v.src = url;
      }
    });
    pv.measured.set(url, p);
    return p;
  }

  async function measureAll() {
    let changed = false;
    for (const c of state.edit) {
      const m = await measure(c.url, c.kind);
      if (m && c.kind === 'video' && m.len && c.len !== m.len) { c.len = m.len; changed = true; }
    }
    if (changed) { saveEdit(); renderEditor(); }
  }

  function addToEdit(url, kind) {
    if (state.edit.length >= MAX_EDIT_CLIPS) { setStatus(`The edit holds ${MAX_EDIT_CLIPS} clips.`, 'warn'); return; }
    const c = normClip({ url, kind: kind || kindOf(url), caption: '' });
    if (!c) return;
    state.edit.push(c);
    state.editSel = state.edit.length - 1;
    saveEdit();
    renderEditor();
    measureAll();
    if (state.autoCaps !== 'off' && c.kind === 'video') loadTranscripts().then(showAutoCap);
    loadFollow();
    const n = state.edit.length;
    setStatus(`Added to the edit (${n} clip${n === 1 ? '' : 's'}). Open Edit to trim and export.`);
  }

  async function uploadClip(file) {
    if (!file) return;
    els.tlNote.textContent = 'Uploading...';
    try {
      if (file.size > MAX_UPLOAD) throw new Error('That file is too large (40 MB max).');
      const up = await uploadData(await fileToDataUrl(file));
      if (up.kind !== 'video' && up.kind !== 'image') throw new Error('Use a video (MP4, MOV, WebM) or a picture.');
      els.tlNote.textContent = '';
      addToEdit(up.url, up.kind);
    } catch (e) {
      els.tlNote.textContent = e.message || 'Upload failed.';
    }
  }

  function renderEditor() {
    els.editCount.hidden = !state.edit.length;
    els.editCount.textContent = String(state.edit.length);
    if (state.editSel >= state.edit.length) state.editSel = state.edit.length - 1;
    renderFrameChips();
    renderCapChips();
    renderFixes();
    renderTimeline();
    renderInspector();
    const total = editTotal();
    if (state.ph > total) state.ph = total;
    els.joinBtn.disabled = !state.edit.length;
    els.forYouTube.disabled = !state.edit.length;
    els.forVertical.disabled = !state.edit.length;
    els.splitBtn.disabled = !state.edit.length;
    els.pvEmpty.hidden = !!state.edit.length;
    els.pvPlay.disabled = !state.edit.length;
    if (!pv.playing) showAt(state.ph);
    updateClock();
  }

  function segButton(label, pressed, onClick) {
    const b = el('button', { type: 'button', class: 'seg-btn', 'aria-pressed': pressed ? 'true' : 'false' }, [label]);
    b.addEventListener('click', onClick);
    return b;
  }

  function renderFrameChips() {
    els.frameChips.replaceChildren(...FRAMES.map(([id, label]) => segButton(label, state.editFrame === id, () => {
      state.editFrame = id;
      saveEdit();
      renderEditor();
      loadFollow();
    })));
    const mode = (fill, follow) => () => { state.editFill = fill; state.editFollow = follow; saveEdit(); renderEditor(); loadFollow(); };
    els.fitChips.replaceChildren(
      segButton('Fit', !state.editFill, mode(false, false)),
      segButton('Fill', state.editFill && !state.editFollow, mode(true, false)),
      segButton('Follow', state.editFill && state.editFollow, mode(true, true)),
    );
    els.fitChips.lastChild.title = 'Crop to the shape and keep the person or subject in frame';
    els.fitChips.hidden = state.editFrame === 'original';
    els.pv.dataset.fill = String(state.editFill && state.editFrame !== 'original');
    sizePreview();
  }

  // Output size: the chosen frame, or the first clip's shape capped at 1920 (as the engine does).
  async function outputSize() {
    if (FRAME_SIZE[state.editFrame]) return FRAME_SIZE[state.editFrame];
    const first = state.edit[0];
    const m = first ? await measure(first.url, first.kind) : null;
    if (!m || !m.w || !m.h) return [1920, 1080];
    const scale = Math.min(1, 1920 / Math.max(m.w, m.h));
    const even = (x) => Math.max(2, Math.floor(Math.round(x) / 2) * 2);
    return [even(m.w * scale), even(m.h * scale)];
  }

  // Fits the preview box to the output shape inside the panel width and a height cap.
  async function sizePreview() {
    const [w, h] = await outputSize();
    const maxW = els.pv.parentElement.clientWidth || 320;
    const maxH = Math.min(window.innerHeight * (window.innerWidth <= 720 ? 0.52 : 0.46), 440);
    const s = Math.min(maxW / w, maxH / h);
    els.pv.style.width = `${Math.round(w * s)}px`;
    els.pv.style.height = `${Math.round(h * s)}px`;
    sizeCaption();
  }

  function sizeCaption() {
    const r = els.pv.getBoundingClientRect();
    const m = Math.min(r.width, r.height);
    els.pvCap.style.fontSize = `${Math.max(9, m * 0.06)}px`;
    els.pvAuto.style.setProperty('--m', `${m}px`);
  }

  // ----- Follow (the crop tracks the subject) -----
  // The engine finds the subject with Apple's Vision framework and returns the camera path; the
  // preview slides the picture the same way (object-position), so what you see is what exports.

  const follow = new Map(); // `${url}|${frame}` -> { axis, keys: [[t, pan]] } or a pending promise

  const followOn = () => state.editFill && state.editFollow && !!FRAME_SIZE[state.editFrame];

  function loadFollow() {
    if (!followOn()) return;
    for (const url of new Set(state.edit.filter((c) => c.kind === 'video').map((c) => c.url))) {
      const key = `${url}|${state.editFrame}`;
      if (follow.has(key)) continue;
      follow.set(key, call('/api/reframe', { method: 'POST', body: { url, frame: state.editFrame } }).then((r) => {
        follow.set(key, r.ok && r.data ? r.data : { axis: 'none', keys: [] });
        if (!pv.playing) showAt(state.ph);
      }).catch(() => follow.set(key, { axis: 'none', keys: [] })));
    }
  }

  // Pan (0..1) at source time t, from the keyframes, as the engine's expression computes it.
  function panAt(keys, t) {
    if (!keys.length) return 0.5;
    if (t < keys[0][0]) return keys[0][1];
    for (let i = 0; i + 1 < keys.length; i++) {
      const [t0, p0] = keys[i];
      const [t1, p1] = keys[i + 1];
      if (t >= t0 && t < t1) return p0 + ((p1 - p0) * (t - t0)) / Math.max(1e-3, t1 - t0);
    }
    return keys[keys.length - 1][1];
  }

  function aimPreview(c, srcT) {
    const f = c && c.kind === 'video' && followOn() ? follow.get(`${c.url}|${state.editFrame}`) : null;
    if (!f || f instanceof Promise || f.axis === 'none') { els.pvVideo.style.objectPosition = ''; return; }
    const p = (panAt(f.keys, srcT) * 100).toFixed(2);
    els.pvVideo.style.objectPosition = f.axis === 'x' ? `${p}% 50%` : `50% ${p}%`;
  }

  function renderTimeline() {
    const hadFocus = els.timeline.contains(document.activeElement);
    const blocks = state.edit.map((c, i) => {
      const d = pieceLen(c);
      const thumb = c.kind === 'image'
        ? el('img', { src: c.url, alt: '', loading: 'lazy', draggable: 'false' })
        : el('video', { src: `${c.url}#t=${(c.from + 0.05).toFixed(2)}`, muted: true, preload: 'metadata', playsinline: true });
      const b = el('div', {
        class: 'tl-clip', role: 'option', tabindex: '0', draggable: 'true',
        'aria-selected': i === state.editSel ? 'true' : 'false',
        'aria-label': `Clip ${i + 1}, ${c.kind}, ${d.toFixed(1)} seconds${c.caption ? `, caption: ${c.caption}` : ''}`,
        style: `flex-grow:${Math.max(d, 0.1).toFixed(3)}`,
      }, [thumb, el('span', { class: 'tl-dur' }, [`${d.toFixed(1)}s`]), c.caption ? el('span', { class: 'tl-txt' }, [c.caption]) : null]);
      b.addEventListener('click', (e) => {
        const r = b.getBoundingClientRect();
        let start = 0;
        for (let j = 0; j < i; j++) start += pieceLen(state.edit[j]);
        state.editSel = i;
        seek(start + Math.min(Math.max(0, (e.clientX - r.left) / r.width), 0.999) * d);
        renderTimeline();
        renderInspector();
      });
      b.addEventListener('keydown', (e) => {
        if (e.key === 'Enter') { state.editSel = i; renderEditor(); focusBlock(i); }
        else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') {
          e.preventDefault();
          const j = Math.min(state.edit.length - 1, Math.max(0, i + (e.key === 'ArrowRight' ? 1 : -1)));
          if (e.altKey) moveClip(i, j); else { state.editSel = j; renderEditor(); }
          focusBlock(state.editSel);
        } else if (e.key === 'Delete' || e.key === 'Backspace') { e.preventDefault(); removeClip(i); focusBlock(Math.min(i, state.edit.length - 1)); }
      });
      b.addEventListener('dragstart', (e) => { e.dataTransfer.setData('text/x-agave-clip', String(i)); e.dataTransfer.effectAllowed = 'move'; b.classList.add('dragging'); });
      b.addEventListener('dragend', () => b.classList.remove('dragging'));
      b.addEventListener('dragover', (e) => { if (e.dataTransfer.types.includes('text/x-agave-clip')) { e.preventDefault(); b.classList.add('drop'); } });
      b.addEventListener('dragleave', () => b.classList.remove('drop'));
      b.addEventListener('drop', (e) => {
        b.classList.remove('drop');
        const from = Number(e.dataTransfer.getData('text/x-agave-clip'));
        if (!Number.isInteger(from)) return;
        e.preventDefault();
        moveClip(from, i);
      });
      return b;
    });
    if (!blocks.length) blocks.push(el('p', { class: 'tl-empty muted' }, ['Clips you add appear here.']));
    els.timeline.replaceChildren(...blocks, els.tlHead);
    els.tlHead.hidden = !state.edit.length;
    if (hadFocus) focusBlock(state.editSel); // keyboard edits keep their place
    updateHead();
  }

  // Updates one block's caption label without rebuilding the strip (typing stays smooth).
  function labelBlock(i) {
    const b = els.timeline.querySelectorAll('.tl-clip')[i];
    const c = state.edit[i];
    if (!b || !c) return;
    let t = b.querySelector('.tl-txt');
    if (c.caption && !t) { t = el('span', { class: 'tl-txt' }); b.append(t); }
    if (t) { if (c.caption) t.textContent = c.caption; else t.remove(); }
    b.setAttribute('aria-label', `Clip ${i + 1}, ${c.kind}, ${pieceLen(c).toFixed(1)} seconds${c.caption ? `, caption: ${c.caption}` : ''}`);
  }

  function focusBlock(i) {
    const b = els.timeline.querySelectorAll('.tl-clip')[i];
    if (b) b.focus();
  }

  function moveClip(from, to) {
    if (from === to || from < 0 || to < 0 || from >= state.edit.length || to >= state.edit.length) return;
    const [c] = state.edit.splice(from, 1);
    state.edit.splice(to, 0, c);
    state.editSel = to;
    saveEdit();
    renderEditor();
  }

  function removeClip(i) {
    state.edit.splice(i, 1);
    state.editSel = Math.min(i, state.edit.length - 1);
    saveEdit();
    renderEditor();
  }

  function splitAtPlayhead() {
    const loc = locate(state.ph);
    if (!loc) return;
    const c = state.edit[loc.i];
    if (loc.local < MIN_PIECE || loc.d - loc.local < MIN_PIECE) {
      els.tlNote.textContent = 'Move the playhead inside a clip to split it.';
      return;
    }
    const a = { ...c };
    const b = { ...c };
    if (c.kind === 'image') { a.to = loc.local; b.to = loc.d - loc.local; } else { a.to = c.from + loc.local; b.from = c.from + loc.local; }
    state.edit.splice(loc.i, 1, a, b);
    state.editSel = loc.i + 1;
    els.tlNote.textContent = '';
    saveEdit();
    renderEditor();
  }

  function renderInspector() {
    const i = state.editSel;
    const c = state.edit[i];
    els.inspector.hidden = !c;
    if (!c) { els.inspector.replaceChildren(); return; }
    const field = (label, input) => el('label', { class: 'field ins-field' }, [el('span', { class: 'k' }, [label]), input]);
    const numIn = (value, min, max, aria) => el('input', { type: 'number', step: '0.1', min: String(min), max: String(max), value: value.toFixed(1), 'aria-label': aria, inputmode: 'decimal' });
    const kids = [el('h3', {}, [`Clip ${i + 1} · ${c.kind === 'image' ? 'picture' : 'video'} · ${pieceLen(c).toFixed(1)} s${c.kind === 'video' && c.len ? ` of ${c.len.toFixed(1)} s` : ''}`])];
    if (c.kind === 'video') {
      const len = c.len ?? Math.max(c.to ?? 0, c.from + MIN_PIECE);
      const end = Math.min(c.to ?? len, len);
      const inStart = numIn(c.from, 0, len, 'Start inside the clip, in seconds');
      const inEnd = numIn(end, 0, len, 'End inside the clip, in seconds');
      inStart.addEventListener('change', () => setTrim(c, Number(inStart.value), null));
      inEnd.addEventListener('change', () => setTrim(c, null, Number(inEnd.value)));
      const atStart = el('button', { type: 'button', class: 'small-btn' }, ['Start at playhead']);
      const atEnd = el('button', { type: 'button', class: 'small-btn' }, ['End at playhead']);
      atStart.addEventListener('click', () => { const l = locate(state.ph); if (l && l.i === i) setTrim(c, c.from + l.local, null); });
      atEnd.addEventListener('click', () => { const l = locate(state.ph); if (l && l.i === i) setTrim(c, null, c.from + l.local); });
      kids.push(el('div', { class: 'ins-row' }, [field('Start (s)', inStart), field('End (s)', inEnd), atStart, atEnd]));
    } else {
      const inLen = numIn(pieceLen(c), MIN_PIECE, STILL_MAX, 'How long the picture shows, in seconds');
      inLen.addEventListener('change', () => {
        const v = Number(inLen.value);
        c.to = Number.isFinite(v) ? Math.min(STILL_MAX, Math.max(0.5, v)) : STILL_DEFAULT;
        saveEdit();
        renderEditor();
      });
      kids.push(el('div', { class: 'ins-row' }, [field('Shows for (s)', inLen)]));
    }
    const cap = el('input', { type: 'text', maxlength: String(MAX_CAPTION), placeholder: 'Caption on this clip (optional)', value: c.caption || '', 'aria-label': `Caption for clip ${i + 1}` });
    cap.addEventListener('input', () => { c.caption = cap.value; saveEdit(); labelBlock(i); showCaption(); });
    kids.push(field('Text', cap));
    const act = (label, fn, disabled) => {
      const b = el('button', { type: 'button', class: 'small-btn', disabled }, [label]);
      b.addEventListener('click', fn);
      return b;
    };
    kids.push(el('div', { class: 'ins-row' }, [
      act('Move left', () => moveClip(i, i - 1), i === 0),
      act('Move right', () => moveClip(i, i + 1), i === state.edit.length - 1),
      act('Duplicate', () => {
        if (state.edit.length >= MAX_EDIT_CLIPS) return;
        state.edit.splice(i + 1, 0, { ...c });
        state.editSel = i + 1;
        saveEdit();
        renderEditor();
      }, state.edit.length >= MAX_EDIT_CLIPS),
      act('Remove', () => removeClip(i)),
    ]));
    els.inspector.replaceChildren(...kids);
  }

  // Keeps a trim inside the source and at least MIN_PIECE long; the playhead stays on that clip.
  function setTrim(c, from, to) {
    const len = c.len ?? Infinity;
    let a = from ?? c.from;
    let b = to ?? Math.min(c.to ?? len, len);
    if (!Number.isFinite(a)) a = c.from;
    if (!Number.isFinite(b)) b = c.to ?? len;
    a = Math.max(0, a);
    b = Math.min(len, b);
    if (from !== null) a = Math.min(a, b - MIN_PIECE);
    if (to !== null) b = Math.max(b, a + MIN_PIECE);
    c.from = Math.max(0, a);
    c.to = Number.isFinite(b) && b < len ? b : null;
    saveEdit();
    renderEditor();
  }

  // ----- preview -----

  function showCaption() {
    const loc = locate(state.ph);
    const t = loc ? (state.edit[loc.i].caption || '').trim() : '';
    els.pvCap.textContent = t;
    els.pvCap.hidden = !t;
  }

  async function showAt(t) {
    const loc = locate(t);
    if (!loc) {
      els.pvVideo.hidden = true;
      els.pvImg.hidden = true;
      els.pvCap.hidden = true;
      return;
    }
    const c = state.edit[loc.i];
    pv.loading = true;
    try {
      if (c.kind === 'image') {
        els.pvVideo.pause();
        els.pvVideo.hidden = true;
        if (els.pvImg.getAttribute('src') !== c.url) els.pvImg.src = c.url;
        els.pvImg.hidden = false;
      } else {
        els.pvImg.hidden = true;
        els.pvVideo.hidden = false;
        els.pvVideo.muted = !(els.keepSound.checked && !state.song);
        if (els.pvVideo.dataset.url !== c.url) {
          els.pvVideo.dataset.url = c.url;
          els.pvVideo.src = c.url;
          await waitFor(els.pvVideo, 'loadedmetadata').catch(() => {});
        }
        els.pvVideo.currentTime = c.from + loc.local;
        aimPreview(c, c.from + loc.local);
      }
    } finally {
      pv.loading = false;
    }
    showCaption();
    updateHead();
  }

  function updateHead() {
    const loc = locate(state.ph);
    const b = loc && els.timeline.querySelectorAll('.tl-clip')[loc.i];
    if (!b) return;
    const x = b.offsetLeft + (loc.d ? loc.local / loc.d : 0) * b.offsetWidth;
    els.tlHead.style.transform = `translateX(${x}px)`;
  }

  function updateClock() {
    els.pvTime.textContent = `${fmtTime(Math.min(state.ph, editTotal()))} / ${fmtTime(editTotal())}`;
    showAutoCap();
  }

  async function seek(t) {
    state.ph = Math.min(Math.max(0, t), editTotal());
    await showAt(state.ph);
    updateClock();
    if (pv.playing) startPiece();
  }

  function startPiece() {
    const loc = locate(state.ph);
    if (loc && state.edit[loc.i].kind === 'video') els.pvVideo.play().catch(() => {});
    pv.last = performance.now();
  }

  async function play() {
    if (!state.edit.length) return;
    if (state.ph >= editTotal() - 0.05) state.ph = 0;
    pv.playing = true;
    els.pvPlay.textContent = '❚❚';
    els.pvPlay.setAttribute('aria-label', 'Pause');
    await showAt(state.ph);
    if (state.song) {
      if (!pv.song || pv.song.dataset.url !== state.song.url) { pv.song = new Audio(state.song.url); pv.song.dataset.url = state.song.url; }
      pv.song.currentTime = state.ph;
      pv.song.play().catch(() => {});
    }
    startPiece();
    requestAnimationFrame(tick);
  }

  function pause() {
    pv.playing = false;
    els.pvVideo.pause();
    if (pv.song) pv.song.pause();
    els.pvPlay.textContent = '▶';
    els.pvPlay.setAttribute('aria-label', 'Play');
  }

  async function tick(now) {
    if (!pv.playing) return;
    if (!pv.loading) {
      const loc = locate(state.ph);
      const c = loc && state.edit[loc.i];
      if (!c) { pause(); return; }
      let done;
      if (c.kind === 'video') {
        const local = els.pvVideo.currentTime - c.from;
        aimPreview(c, els.pvVideo.currentTime);
        state.ph = loc.start + Math.min(Math.max(0, local), loc.d);
        done = local >= loc.d - 0.04 || els.pvVideo.ended;
      } else {
        state.ph = Math.min(loc.start + loc.d, state.ph + (now - pv.last) / 1000);
        done = state.ph >= loc.start + loc.d - 0.001;
      }
      pv.last = now;
      if (done) {
        const next = loc.start + loc.d;
        if (loc.i >= state.edit.length - 1) {
          state.ph = editTotal();
          pause();
          updateHead();
          updateClock();
          return;
        }
        state.ph = next + 0.001;
        await showAt(state.ph);
        startPiece();
      }
      updateHead();
      updateClock();
    }
    requestAnimationFrame(tick);
  }

  // ----- auto captions and pauses -----
  // Transcripts come from the engine (Parakeet or whisper.cpp on this Mac), one per source file, in source time.
  // Captions are worked out from the current timeline every time, so trims, splits and cut pauses
  // never leave a caption on the wrong picture.

  const CAP_STYLES = [['off', 'Off'], ['simple', 'Simple'], ['pop', 'Word by word']];
  const PAUSE_PAD = 0.12; // silence kept on each side of a cut, so speech is not clipped
  const MIN_KEEP = 0.15;
  let capsLoading = null;

  function renderCapChips() {
    els.autoCapChips.replaceChildren(...CAP_STYLES.map(([id, label]) => segButton(label, state.autoCaps === id, async () => {
      state.autoCaps = id;
      saveEdit();
      renderCapChips();
      if (id !== 'off') await loadTranscripts();
      showAutoCap();
    })));
  }

  // Fetches the transcript of every video source on the timeline that doesn't have one yet.
  function loadTranscripts() {
    if (capsLoading) return capsLoading;
    capsLoading = (async () => {
      const urls = [...new Set(state.edit.filter((c) => c.kind === 'video').map((c) => c.url))].filter((u) => !state.transcripts.has(u));
      let failed = 0;
      for (let k = 0; k < urls.length; k++) {
        els.capNote.textContent = `Listening to clip ${k + 1} of ${urls.length}...`;
        let r;
        try { r = await call('/api/transcribe', { method: 'POST', body: { url: urls[k] } }); } catch (_) { r = { ok: false, status: 0 }; }
        if (r.ok && r.data && r.data.transcript) {
          state.transcripts.set(urls[k], r.data.transcript.words || []);
        } else if (r.status === 503) {
          const b = el('button', { type: 'button', class: 'small-btn' }, ['Set up captions']);
          b.addEventListener('click', () => openSetup(2));
          els.capNote.replaceChildren('Captions are not set up yet: they need a speech model (a free download) and, for Whisper, whisper-cli. ', b);
          return;
        } else {
          failed++;
        }
      }
      const n = timelineWords().length;
      els.capNote.textContent = failed
        ? `${failed} clip${failed === 1 ? '' : 's'} could not be transcribed.`
        : n ? `${n} words found.` : 'No speech found in these clips.';
    })().finally(() => { capsLoading = null; });
    return capsLoading;
  }

  // Applies the user's word fixes ("bamboo" -> "Bambu"), keeping punctuation; an empty fix hides the word.
  function fixWord(text) {
    if (!state.wordFixes.length) return text;
    const m = text.match(/^([^\p{L}\p{N}']*)(.*?)([^\p{L}\p{N}']*)$/u);
    const core = m ? m[2] : text;
    const f = state.wordFixes.find((x) => x.from.toLowerCase() === core.toLowerCase());
    if (!f) return text;
    return f.to ? `${m[1]}${f.to}${m[3]}` : '';
  }

  function renderFixes() {
    els.fixList.replaceChildren(...state.wordFixes.map((f, i) => {
      const rm = el('button', { type: 'button', class: 'link-btn', 'aria-label': `Remove the fix for ${f.from}` }, ['Remove']);
      rm.addEventListener('click', () => { state.wordFixes.splice(i, 1); saveEdit(); renderFixes(); showAutoCap(); });
      return el('li', {}, [el('span', {}, [`${f.from} → ${f.to || '(hidden)'}`]), rm]);
    }));
  }

  function addFix(e) {
    e.preventDefault();
    const from = els.fixFrom.value.trim();
    const to = els.fixTo.value.trim();
    if (!from || /\s/.test(from)) { els.capNote.textContent = 'Fix one word at a time.'; return; }
    if (state.wordFixes.length >= 50) { els.capNote.textContent = 'That is the most word fixes one edit can hold (50).'; return; }
    state.wordFixes = state.wordFixes.filter((f) => f.from.toLowerCase() !== from.toLowerCase());
    state.wordFixes.push({ from, to });
    els.fixFrom.value = '';
    els.fixTo.value = '';
    els.capNote.textContent = '';
    saveEdit();
    renderFixes();
    showAutoCap();
  }

  // Every spoken word that survives the current trims, in timeline time.
  function timelineWords() {
    const out = [];
    let start = 0;
    for (const c of state.edit) {
      const d = pieceLen(c);
      const words = c.kind === 'video' ? state.transcripts.get(c.url) : null;
      if (words) {
        const end = c.from + d;
        for (const w of words) {
          const mid = (w.t0 + w.t1) / 2;
          if (mid < c.from || mid >= end) continue;
          const text = fixWord(w.text);
          if (text) out.push({ t0: start + Math.max(w.t0, c.from) - c.from, t1: start + Math.min(w.t1, end) - c.from, text });
        }
      }
      start += d;
    }
    return out;
  }

  // Groups words into short caption lines: at most 3 words or about 18 letters, broken at
  // sentence ends and at gaps in speech.
  function captionChunks(words) {
    const chunks = [];
    let cur = null;
    for (const w of words) {
      const chars = cur ? cur.words.reduce((s, x) => s + x.text.length + 1, 0) + w.text.length : 0;
      const prev = cur && cur.words[cur.words.length - 1];
      if (!cur || cur.words.length >= 3 || chars > 18 || w.t0 - prev.t1 > 0.5 || /[.!?]$/.test(prev.text)) {
        cur = { words: [] };
        chunks.push(cur);
      }
      cur.words.push(w);
    }
    chunks.forEach((c, i) => {
      c.start = c.words[0].t0;
      c.end = c.words[c.words.length - 1].t1;
      const next = chunks[i + 1];
      if (next && next.words[0].t0 - c.end < 0.4) c.end = next.words[0].t0; // no flicker between close lines
    });
    return chunks.filter((c) => c.end - c.start >= 0.05);
  }

  // What is on screen and when: one frame per line (Simple) or per word (Word by word, current word lit).
  function captionFrames() {
    if (state.autoCaps === 'off') return [];
    const frames = [];
    for (const c of captionChunks(timelineWords())) {
      if (state.autoCaps === 'simple') { frames.push({ start: c.start, end: c.end, words: c.words.map((w) => w.text), hi: -1 }); continue; }
      c.words.forEach((w, j) => {
        const start = j === 0 ? c.start : w.t0;
        const end = j === c.words.length - 1 ? c.end : c.words[j + 1].t0;
        if (end - start >= 0.03) frames.push({ start, end, words: c.words.map((x) => x.text), hi: j });
      });
    }
    // Keep the list forwards and free of overlaps (the engine checks this too).
    for (let i = 1; i < frames.length; i++) frames[i].start = Math.max(frames[i].start, frames[i - 1].end);
    return frames.filter((f) => f.end - f.start >= 0.03);
  }

  function showAutoCap() {
    if (state.autoCaps === 'off') { els.pvAuto.hidden = true; return; }
    const f = captionFrames().find((x) => state.ph >= x.start && state.ph < x.end);
    els.pvAuto.hidden = !f;
    if (!f) return;
    els.pvAuto.classList.toggle('pop', state.autoCaps === 'pop');
    els.pvAuto.replaceChildren(...f.words.flatMap((t, j) => [j ? ' ' : '', el('span', { class: j === f.hi ? 'hi' : '' }, [t])]));
  }

  // Draws one caption frame as a transparent PNG at the output size (same look as the preview).
  function autoCapPng(w, h, f) {
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const ctx = c.getContext('2d');
    const pop = state.autoCaps === 'pop';
    const fs = Math.round(Math.min(w, h) * (pop ? 0.085 : 0.065));
    ctx.font = `${pop ? 800 : 600} ${fs}px Geist, system-ui, sans-serif`;
    ctx.textBaseline = 'middle';
    ctx.lineJoin = 'round';
    const words = f.words.map((t) => (pop ? t.toUpperCase() : t));
    const space = ctx.measureText(' ').width;
    const lines = [[]];
    let lw = 0;
    words.forEach((t, j) => {
      const ww = ctx.measureText(t).width;
      if (lines[lines.length - 1].length && lw + space + ww > w * 0.86) { lines.push([]); lw = 0; }
      lines[lines.length - 1].push({ t, j, ww });
      lw += (lw ? space : 0) + ww;
    });
    const lh = fs * 1.2;
    let y = h * 0.72 - ((lines.length - 1) * lh) / 2;
    for (const line of lines) {
      const total = line.reduce((s, x) => s + x.ww, 0) + space * (line.length - 1);
      let x = (w - total) / 2;
      for (const word of line) {
        ctx.lineWidth = fs * (pop ? 0.24 : 0.2);
        ctx.strokeStyle = 'rgba(8, 8, 10, 0.9)';
        ctx.strokeText(word.t, x, y);
        ctx.fillStyle = word.j === f.hi ? '#ff6b3d' : '#f4f2ee';
        ctx.fillText(word.t, x, y);
        x += word.ww + space;
      }
      y += lh;
    }
    return c.toDataURL('image/png');
  }

  // Uploads every caption frame (four at a time) and returns the engine's timed list.
  async function timedCaptions(w, h) {
    const frames = captionFrames();
    const out = new Array(frames.length);
    let next = 0;
    let done = 0;
    const worker = async () => {
      while (next < frames.length) {
        const i = next++;
        const up = await uploadData(autoCapPng(w, h, frames[i]));
        out[i] = { url: up.url, start: Number(frames[i].start.toFixed(3)), end: Number(frames[i].end.toFixed(3)) };
        done++;
        els.editNote.textContent = `Drawing captions ${done} of ${frames.length}...`;
      }
    };
    await Promise.all([worker(), worker(), worker(), worker()]);
    return out;
  }

  // Cuts the quiet gaps out of every video clip, keeping a little silence around speech.
  async function removePauses() {
    const urls = [...new Set(state.edit.filter((c) => c.kind === 'video').map((c) => c.url))];
    if (!urls.length) { els.tlNote.textContent = 'Add a video clip first.'; return; }
    pause();
    els.pausesBtn.disabled = true;
    els.tlNote.textContent = 'Finding pauses...';
    try {
      const found = new Map();
      for (const u of urls) {
        const r = await call('/api/pauses', { method: 'POST', body: { url: u, threshold_db: -35, min_pause: 0.5 } });
        found.set(u, r.ok ? r.data : null);
      }
      const before = JSON.stringify(state.edit);
      const out = [];
      let cut = 0;
      let places = 0;
      let quiet = 0;
      for (const c of state.edit) {
        const d = c.kind === 'video' ? found.get(c.url) : null;
        if (!d || !d.has_audio) { if (c.kind === 'video') quiet++; out.push(c); continue; }
        const end = c.from + pieceLen(c);
        const keep = [];
        let at = c.from;
        for (const [a, b] of d.pauses) {
          const s = Math.max(a + PAUSE_PAD, c.from);
          const e = Math.min(b - PAUSE_PAD, end);
          if (e - s < MIN_KEEP) continue;
          if (s > at) keep.push([at, s]);
          at = Math.max(at, e);
        }
        if (at < end) keep.push([at, end]);
        const kept = keep.filter(([a, b]) => b - a >= MIN_KEEP);
        if (!kept.length) { quiet++; out.push(c); continue; } // all quiet: leave the clip alone
        const left = kept.reduce((s, [a, b]) => s + b - a, 0);
        if (end - c.from - left < 0.05) { out.push(c); continue; }
        cut += end - c.from - left;
        places += kept.length - 1 + (kept[0][0] > c.from + 0.01 ? 1 : 0) + (kept[kept.length - 1][1] < end - 0.01 ? 1 : 0);
        kept.forEach(([a, b]) => out.push({ ...c, from: a, to: b, len: c.len ?? d.len }));
      }
      if (out.length > MAX_EDIT_CLIPS) { els.tlNote.textContent = `That would make more than ${MAX_EDIT_CLIPS} pieces. Trim the clips first.`; return; }
      if (!cut) {
        els.tlNote.textContent = quiet ? 'No sound to work from in these clips.' : 'No pauses longer than half a second.';
        return;
      }
      state.edit = out;
      state.undo = before;
      state.undoAfter = JSON.stringify(out);
      state.editSel = Math.min(state.editSel, out.length - 1);
      saveEdit();
      renderEditor();
      els.undoBtn.hidden = false;
      els.tlNote.textContent = `Cut ${cut.toFixed(1)} s of pauses in ${places} place${places === 1 ? '' : 's'}.`;
    } finally {
      els.pausesBtn.disabled = false;
    }
  }

  function undoEdit() {
    if (!state.undo) return;
    try { state.edit = JSON.parse(state.undo).map(normClip).filter(Boolean); } catch (_) { return; }
    state.undo = null;
    els.undoBtn.hidden = true;
    els.tlNote.textContent = 'Undone.';
    saveEdit();
    renderEditor();
  }

  function wrapLines(ctx, text, max) {
    const words = text.split(/\s+/).filter(Boolean);
    const lines = [];
    let line = '';
    for (const w of words) {
      const t = line ? `${line} ${w}` : w;
      if (ctx.measureText(t).width > max && line) { lines.push(line); line = w; } else line = t;
    }
    if (line) lines.push(line);
    return lines;
  }

  // Draws a caption as a transparent PNG at the output size; the engine lays it over the framed clip.
  function captionPng(w, h, text) {
    const c = document.createElement('canvas');
    c.width = w;
    c.height = h;
    const ctx = c.getContext('2d');
    const fs = Math.round(Math.min(w, h) * 0.06);
    ctx.font = `600 ${fs}px Geist, system-ui, sans-serif`;
    ctx.textAlign = 'center';
    ctx.textBaseline = 'bottom';
    ctx.lineJoin = 'round';
    const lines = wrapLines(ctx, text, w * 0.84);
    let y = h * 0.9 - (lines.length - 1) * fs * 1.2;
    for (const l of lines) {
      ctx.lineWidth = fs * 0.2;
      ctx.strokeStyle = 'rgba(8, 8, 10, 0.85)';
      ctx.strokeText(l, w / 2, y);
      ctx.fillStyle = '#f4f2ee';
      ctx.fillText(l, w / 2, y);
      y += fs * 1.2;
    }
    return c.toDataURL('image/png');
  }

  // Platform presets: set the shape and sound for where the video is going, then export.
  function exportFor(where) {
    if (!state.edit.length) return;
    if (where === 'youtube') {
      state.editFrame = '16:9';
      state.editFill = false;
      state.editFollow = false;
    } else {
      state.editFrame = '9:16';
      state.editFill = true;
      state.editFollow = true;
    }
    els.loudness.checked = true;
    saveEdit();
    renderEditor();
    loadFollow();
    joinClips().then(() => {
      // YouTube counts vertical videos up to 3 minutes as Shorts (its 2024 change).
      if (where === 'vertical' && editTotal() > 180) setStatus('Exporting. Note: this is over 3 minutes, so YouTube will not treat it as a Short.', 'warn');
    });
  }

  async function joinClips() {
    if (!state.edit.length) return;
    pause();
    els.joinBtn.disabled = true;
    els.editNote.textContent = 'Preparing...';
    try {
      const [w, h] = await outputSize();
      if (state.autoCaps !== 'off') await loadTranscripts();
      const timed = state.autoCaps === 'off' ? [] : await timedCaptions(w, h);
      const captions = [];
      for (const c of state.edit) {
        const t = (c.caption || '').trim();
        captions.push(t ? (await uploadData(captionPng(w, h, t))).url : null);
      }
      const body = {
        clips: state.edit.map((c) => (c.kind === 'image' ? { url: c.url, from: 0, to: pieceLen(c) } : { url: c.url, from: c.from, to: c.to })),
        captions,
        audio: state.song ? state.song.url : null,
        keep_clip_audio: els.keepSound.checked,
        frame: state.editFrame === 'original' ? null : state.editFrame,
        fill: state.editFill,
        timed_captions: timed,
        follow: state.editFollow,
        clean_voice: els.cleanVoice.checked,
        loudness: els.loudness.checked,
      };
      const r = await call('/api/assemble', { method: 'POST', body });
      if (r.status === 202 && r.data && r.data.job) {
        upsertJob(r.data.job);
        els.editNote.textContent = 'Exporting. The finished video appears in the gallery.';
        closePanel();
        setStatus('Exporting the edit...');
        refreshJobs();
      } else {
        const code = r.data && r.data.error;
        els.editNote.textContent = code === 'invalid_clip' ? 'A clip is missing or its trim is empty. Check the timeline.' : (code || `Could not start (${r.status}).`);
      }
    } catch (e) {
      els.editNote.textContent = e.message || 'Could not start.';
    } finally {
      els.joinBtn.disabled = !state.edit.length;
    }
  }

  async function pickSong(file) {
    if (!file) return;
    els.songName.textContent = 'Uploading...';
    try {
      if (file.size > MAX_UPLOAD) throw new Error('That file is too large (40 MB max).');
      const up = await uploadData(await fileToDataUrl(file));
      if (up.kind !== 'audio' && up.kind !== 'video') throw new Error('Use an audio file (MP3, WAV, M4A) or an MP4.');
      state.song = { url: up.url, name: file.name };
    } catch (e) {
      state.song = null;
      els.editNote.textContent = e.message || 'Upload failed.';
    }
    renderSong();
    saveEdit();
  }

  function renderSong() {
    els.songName.textContent = state.song ? state.song.name : 'None';
    els.clearSong.hidden = !state.song;
  }

  // ---------- prompt library ----------

  async function loadLibrary() {
    if (!state.library) {
      try {
        const r = await fetch('/app/library.json', { cache: 'no-cache' });
        state.library = r.ok ? ((await r.json()).items || []) : [];
      } catch (_) { state.library = []; }
    }
    renderLibrary();
  }

  function renderLibrary() {
    const items = state.library || [];
    const cats = ['All', ...new Set(items.map((i) => i.category))];
    els.libCats.textContent = '';
    for (const c of cats) {
      const b = el('button', { type: 'button', class: 'lib-cat', 'aria-pressed': String(c === state.libCat) }, [c]);
      b.addEventListener('click', () => { state.libCat = c; renderLibrary(); });
      els.libCats.append(b);
    }
    els.libGrid.textContent = '';
    const shown = items.filter((i) => state.libCat === 'All' || i.category === state.libCat);
    if (!shown.length) els.libGrid.append(el('p', { class: 'muted' }, ['The library could not be loaded.']));
    for (const it of shown) {
      const m = state.models.find((x) => x.id === it.model);
      const meta = [m ? m.name : it.model, it.aspect, it.recipe ? 'recipe' : null].filter(Boolean).join(' · ');
      const use = el('button', { type: 'button', class: 'small-btn' }, ['Use this']);
      use.addEventListener('click', () => useLibrary(it));
      const copy = el('button', { type: 'button', class: 'link-btn' }, ['Copy']);
      copy.addEventListener('click', async () => {
        try { await navigator.clipboard.writeText(it.prompt); copy.textContent = 'Copied'; } catch (_) { copy.textContent = 'Select and copy'; }
      });
      els.libGrid.append(el('article', { class: 'lib-card' }, [
        it.preview
          ? el('div', { class: 'lib-thumb' + (it.preview_kind === 'video' ? ' is-video' : '') }, [
            el('img', { src: '/app/' + it.preview, alt: it.title, loading: 'lazy', decoding: 'async' }),
          ])
          : libShape(it),
        el('div', { class: 'lib-body' }, [
          el('span', { class: 'lib-cat-tag' }, [it.category]),
          el('h3', {}, [it.title]),
          el('p', { class: 'muted' }, [it.blurb]),
          el('p', { class: 'lib-meta' }, [meta]),
          it.prompt ? el('details', { class: 'lib-prompt' }, [el('summary', {}, ['Prompt']), el('p', {}, [it.prompt]), copy]) : el('p', { class: 'lib-meta' }, ['Built-in recipe prompt']),
          it.preview_note ? el('p', { class: 'lib-meta' }, [it.preview_note]) : null,
          use,
        ]),
      ]));
    }
  }

  // A drawn card for entries without a preview: the output's shape, split into the panels a recipe makes.
  function libShape(it) {
    const cells = { grid4: 4, product: 9, sheet: 3 }[it.recipe] || 1;
    const cols = { 4: 2, 9: 3, 3: 3 }[cells] || 1;
    const [w, h] = (it.aspect || (it.recipe === 'sheet' ? '16:9' : '1:1')).split(':').map(Number);
    const frame = el('div', { class: 'lib-frame', style: `aspect-ratio: ${w} / ${h}; grid-template-columns: repeat(${cols}, 1fr)` },
      Array.from({ length: cells }, () => el('i', {})));
    const label = { image: 'Image', video: 'Video', '3d': '3D' }[it.mode] || it.mode;
    return el('div', { class: 'lib-thumb is-drawn', 'aria-hidden': 'true' }, [
      frame,
      el('span', { class: 'lib-shape-tag' }, [[label, it.aspect].filter(Boolean).join(' · ')]),
    ]);
  }

  // Fills the composer from a library entry. Nothing is sent until Generate.
  function useLibrary(it) {
    closePanel(true);
    state.libModel = it.model;
    setMode(it.mode);
    if (!it.needs_ref) {
      state.refs = [];
      els.ref.value = '';
      renderTray();
      syncRefMode();
    }
    useModel(state.models.find((m) => m.id === it.model));
    els.recipe.value = it.recipe || '';
    if (it.aspect && [...$('#aspect').options].some((o) => o.value === it.aspect)) {
      $('#aspect').value = it.aspect;
      state.chips.aspect = it.aspect;
      renderCost();
    }
    els.prompt.value = it.prompt || '';
    autosize();
    els.prompt.focus();
    if (it.needs_ref && !refActive()) setStatus(it.needs_ref + ' Then press Generate.', 'warn');
    else setStatus(`${it.title} is ready. Check the cost, then press Generate.`);
  }

  // ---------- board ----------
  // The Board view (board.js) loads the vendored Excalidraw editor the first time it opens. Its drawing stays in the
  // page while the studio shows, so unsaved changes survive switching away. Send to Video only fills the form.

  let boardMod = null;
  const boardBridge = {
    call,
    jobs: () => state.jobs,
    jobName: jobModelName,
    sendToVideo(shot) {
      state.libModel = null;
      state.refs = shot.image ? [shot.image] : [];
      els.ref.value = '';
      els.recipe.value = '';
      setMode('video');
      renderTray();
      syncRefMode();
      els.prompt.value = shot.prompt || '';
      autosize();
      els.prompt.focus();
      setStatus(shot.image
        ? `${shot.name} is in the Video form with its start picture. Check the model and the cost, then press Generate.`
        : `${shot.name} is in the Video form. Its frame has no picture, so this is text to video. Check the model and the cost, then press Generate.`);
    },
  };

  function showBoard() {
    closePanel(true);
    els.lookAsk.hidden = true;
    state.view = 'board';
    els.shell.classList.add('board-on');
    els.board.hidden = false;
    document.querySelectorAll('.mode[data-mode]').forEach((b) => b.setAttribute('aria-pressed', 'false'));
    els.boardBtn.setAttribute('aria-pressed', 'true');
    els.title.textContent = 'Board';
    els.sub.textContent = 'Sketch shots, drop finished results in, and frame each shot to send it to Video.';
    document.title = 'Board · Agave Studio';
    history.replaceState(null, '', '#board');
    import('/app/board.js')
      .then((m) => { boardMod = m; return m.show(els.board, boardBridge); })
      .catch(() => { els.board.textContent = 'The board could not load. Reload the page.'; });
  }

  function leaveBoard() {
    if (state.view !== 'board') return;
    state.view = 'studio';
    els.shell.classList.remove('board-on');
    els.board.hidden = true;
    els.boardBtn.setAttribute('aria-pressed', 'false');
  }

  // ---------- pipelines ----------
  // A pipeline runs the studio's own steps in a row on the engine (data/pipelines.json), so a run survives a
  // reload. The full price shows before it starts; a step that costs money waits for a click here.

  const pipe = { list: null, runs: [], view: null, image: null, subject: '', tag: '', timer: null, editSeen: new Set(), busy: false };
  const RUN_LABEL = { waiting: 'Waiting for you', running: 'Running', failed: 'Failed', done: 'Finished' };
  const STEP_LABEL = { pending: 'Next', waiting: 'Waiting for your click', running: 'Running', done: 'Done', failed: 'Failed' };
  const STEP_ERRORS = {
    missing_key: (s) => `Add ${s.key || 'the provider key'} in Settings, then press Run again.`,
    still_first: () => 'Picture before video is on in Settings, and this step makes video from words. Turn it off, then press Run again.',
    too_many_jobs: () => 'Too many jobs are running. Wait for some to finish, then press Run again.',
    adapter_not_implemented: () => 'This provider is not connected yet.',
  };

  const cents = (c) => `$${(c / 100).toFixed(2)}`;
  // Element.append turns null into the text "null", so empty parts are dropped first.
  const appendAll = (parent, kids) => parent.append(...kids.filter((k) => k !== null && k !== undefined && k !== false));
  // "≈ $0.39 + 30 Meshy cr", "Free", from a pipeline total.
  function totalText(t) {
    if (!t) return '';
    const parts = [];
    if (t.cents) parts.push(`≈ ${cents(t.cents)}`);
    for (const c of t.credits || []) parts.push(`${c.credits} ${c.vendor} cr`);
    if (t.unpriced) parts.push(`${t.unpriced} step${t.unpriced === 1 ? '' : 's'} with no published price`);
    return parts.length ? parts.join(' + ') : 'Free';
  }
  function stepPrice(s) {
    const p = s.price || {};
    if (!s.paid) return 'Free';
    if (p.cents) return `≈ ${cents(p.cents)}`;
    if (p.credits) return `${p.credits} ${p.credits_vendor || ''} cr`.trim();
    return 'No published price';
  }

  // A button that spends money: ember, with the cost first. Free actions stay quiet.
  function actionButton(step, verb, onClick, id) {
    const paid = step && step.paid;
    const b = paid
      ? el('button', { type: 'button', class: 'pill pay', id }, [`Pay ${stepPrice(step)} · ${verb}`])
      : el('button', { type: 'button', class: 'pill quiet', id }, [verb.charAt(0).toUpperCase() + verb.slice(1)]);
    if (paid) b.title = 'Spends money on your own key at the provider\'s list price';
    b.addEventListener('click', () => onClick(b));
    return b;
  }

  async function loadPipelines() {
    if (!pipe.list) {
      try {
        const r = await call('/api/pipelines');
        if (r.ok) pipe.list = (r.data && r.data.pipelines) || [];
      } catch (_) { /* offline */ }
    }
    await loadRuns();
    renderPipeGrid();
  }

  async function loadRuns() {
    try {
      const r = await call('/api/pipelines/runs');
      if (!r.ok) return;
      const before = new Map(pipe.runs.map((x) => [x.id, x.status + x.current]));
      pipe.runs = (r.data && r.data.runs) || [];
      const changed = pipe.runs.some((x) => before.get(x.id) !== x.status + x.current);
      // A finished "Add to edit" step changed the saved timeline on the engine: load it.
      for (const run of pipe.runs) {
        run.steps.forEach((s, i) => {
          const k = `${run.id}:${i}`;
          if (s.kind === 'edit' && s.status === 'done' && !pipe.editSeen.has(k)) {
            if (pipe.editSeen.size || before.size) loadEditServer();
            pipe.editSeen.add(k);
          }
        });
      }
      if (changed) refreshJobs();
    } catch (_) { /* offline: keep what we have */ }
    renderRunChip();
    if (els.prompts.classList.contains('open')) renderRunList();
    if (pipe.view && pipe.view.run && els.pipeline.classList.contains('open')) {
      const fresh = pipe.runs.find((x) => x.id === pipe.view.run.id);
      // Redraw only on a change, so playing previews are not restarted on every check.
      if (fresh && JSON.stringify(fresh) !== JSON.stringify(pipe.view.run)) { pipe.view.run = fresh; renderPipeline(); }
    }
    schedulePipes();
  }

  function schedulePipes() {
    clearTimeout(pipe.timer);
    const live = pipe.runs.some((r) => r.status === 'running');
    if (live) pipe.timer = setTimeout(loadRuns, 2000);
  }

  function renderRunChip() {
    // A run of a pipeline that is no longer in the studio can never go on, so it never asks for attention.
    const open = pipe.runs.filter((r) => r.status !== 'done' && !r.removed);
    const top = open.find((r) => r.status === 'waiting') || open.find((r) => r.status === 'failed') || open[0];
    els.runChip.hidden = !top;
    if (!top) return;
    const n = top.current + 1;
    els.runChipText.textContent = top.status === 'waiting' ? `Pipeline: step ${n} waits for you`
      : top.status === 'failed' ? `Pipeline: step ${n} failed` : `Pipeline: step ${n} running`;
    els.runChip.dataset.state = top.status;
    els.runChip.onclick = () => openRun(top.id);
  }

  function flowStrip(steps) {
    const strip = el('ol', { class: 'pipe-flow', 'aria-label': 'Steps' });
    steps.forEach((s) => strip.append(el('li', { class: s.paid ? 'paid' : 'free', title: `${s.title}: ${stepPrice(s)}` }, [s.title])));
    return strip;
  }

  function renderPipeGrid() {
    els.pipeGrid.textContent = '';
    if (!pipe.list) {
      els.pipeGrid.append(el('p', { class: 'muted small' }, ['Pipelines could not be loaded.']));
      return;
    }
    for (const p of pipe.list) {
      const go = el('button', { type: 'button', class: 'small-btn' }, ['Set up']);
      go.addEventListener('click', () => openSetupView(p.id));
      els.pipeGrid.append(el('article', { class: 'pipe-card' }, [
        flowStrip(p.steps),
        el('h4', {}, [p.title]),
        el('p', { class: 'muted' }, [p.blurb]),
        el('div', { class: 'pipe-card-foot' }, [
          el('span', { class: 'pipe-price' }, [totalText(p.total)]),
          go,
        ]),
      ]));
    }
    renderRunList();
  }

  function renderRunList() {
    els.runList.textContent = '';
    const runs = pipe.runs.slice(0, 6);
    els.runList.hidden = !runs.length;
    if (!runs.length) return;
    els.runList.append(el('h4', {}, ['Your runs']));
    for (const r of runs) {
      const b = el('button', { type: 'button', class: 'run-row', 'data-state': r.status }, [
        el('span', { class: 'run-dot', 'aria-hidden': 'true' }),
        el('span', { class: 'run-row-title' }, [r.title || (r.removed ? 'Removed pipeline' : r.pipeline)]),
        el('span', { class: 'run-row-sub' }, [`${r.subject} · @${r.tag}`]),
        el('span', { class: 'run-row-state' }, [r.status === 'done' ? RUN_LABEL.done : `${RUN_LABEL[r.status] || r.status} · step ${r.current + 1}`]),
      ]);
      b.addEventListener('click', () => openRun(r.id));
      els.runList.append(b);
    }
  }

  function openSetupView(id) {
    const p = (pipe.list || []).find((x) => x.id === id);
    if (!p) return;
    const fromComposer = state.refs.find((u) => u.startsWith('/outputs/') && /\.(png|jpe?g|webp)$/i.test(u));
    pipe.view = { pipeline: p, run: null, note: '' };
    pipe.image = p.needs_image ? (fromComposer || null) : null;
    pipe.subject = '';
    pipe.tag = p.tag;
    openPanel(els.pipeline, renderPipeline);
  }

  function openRun(id) {
    const run = pipe.runs.find((x) => x.id === id);
    if (!run) return;
    pipe.view = { pipeline: (pipe.list || []).find((x) => x.id === run.pipeline) || null, run, note: '' };
    openPanel(els.pipeline, renderPipeline);
    loadRuns();
  }

  // A video shows its first frame as a still, so a thumbnail is never caught mid-frame. Only its start is fetched
  // until it plays.
  function stillVideo(url, label) {
    const v = el('video', { src: `${url}#t=0.05`, muted: true, loop: true, playsinline: true, preload: 'metadata', 'aria-label': label });
    v.muted = true;
    return v;
  }

  // A small thumbnail draws the video's first frame, whole (16:9 box, letterboxed), onto a canvas with high-quality
  // scaling: the browser's own video scaling turns a frame shrunk ten times into noise.
  function frameStill(v) {
    const draw = () => {
      if (v.readyState < 2 || !v.videoWidth || !v.isConnected) return;
      const c = el('canvas', { width: '192', height: '108', role: 'img', 'aria-label': v.getAttribute('aria-label') || '' });
      const ctx = c.getContext('2d');
      if (!ctx) return;
      ctx.imageSmoothingEnabled = true;
      ctx.imageSmoothingQuality = 'high';
      const k = Math.min(c.width / v.videoWidth, c.height / v.videoHeight);
      const w = v.videoWidth * k;
      const h = v.videoHeight * k;
      ctx.drawImage(v, (c.width - w) / 2, (c.height - h) / 2, w, h);
      v.replaceWith(c);
      v.removeAttribute('src');
      v.load();
    };
    v.addEventListener('seeked', draw);
    v.addEventListener('loadeddata', draw);
  }

  // Plays a still video while the pointer is over its card or the card has keyboard focus.
  function playOnHover(host, v) {
    if (reduceMotion) return;
    const play = () => { v.play().catch(() => {}); };
    const stop = () => { if (host.matches(':hover') || host.contains(document.activeElement)) return; v.pause(); v.currentTime = 0.05; };
    host.addEventListener('mouseenter', play);
    host.addEventListener('focusin', play);
    host.addEventListener('mouseleave', stop);
    host.addEventListener('focusout', () => requestAnimationFrame(stop));
  }

  function outThumb(s) {
    const o = (s.outputs || []).find((x) => x.kind === 'video') || (s.outputs || []).find((x) => x.kind === 'image');
    if (!o) return null;
    const media = o.kind === 'video'
      ? stillVideo(o.url, `${s.title} result`)
      : el('img', { src: o.url, alt: `${s.title} result`, loading: 'lazy', decoding: 'async' });
    const a = el('a', { class: `pipe-thumb ${o.kind}`, href: o.url, target: '_blank', rel: 'noopener noreferrer', title: 'Open file' }, [media]);
    if (o.kind === 'video') frameStill(media);
    return a;
  }

  function failedActions(s, i, onRetry) {
    const row = el('div', { class: 'pipe-step-acts' }, [actionButton(s, 'run again', (b) => onRetry(i, b), 'pipeGo')]);
    if (s.error === 'missing_key') {
      const k = el('button', { type: 'button', class: 'small-btn' }, ['Open Settings']);
      k.addEventListener('click', () => openSettings(s.key));
      row.append(k);
    }
    return row;
  }

  function stepError(s) {
    const fix = STEP_ERRORS[s.error];
    return fix ? fix(s) : s.error;
  }

  function stepList(steps, run, onRetry) {
    const list = el('ol', { class: 'pipe-steps' });
    steps.forEach((s, i) => {
      const status = run ? s.status : 'pending';
      const what = s.model_name ? `${s.model_name}${s.vendor ? ' · ' + s.vendor : ''}` : (s.detail || '');
      const meta = [!run && s.paid ? `${what} · waits for your click` : what];
      const thumb = status === 'done' ? outThumb(s) : null;
      const row = el('li', { class: `pipe-step ${status}${run && i === run.current && run.status !== 'done' ? ' current' : ''}${thumb ? '' : ' no-thumb'}` }, [
        el('span', { class: 'pipe-num', 'aria-hidden': 'true' }, [status === 'done' ? '✓' : String(i + 1)]),
        el('div', { class: 'pipe-step-main' }, [
          el('div', { class: 'pipe-step-top' }, [
            el('span', { class: 'pipe-step-title' }, [s.title]),
            s.kind === 'removed' ? null : el('span', { class: 'pipe-step-price' + (s.paid ? ' paid' : ''), title: s.paid ? 'Costs money: waits for your click' : 'Free: runs on its own' }, [stepPrice(s)]),
          ]),
          el('p', { class: 'pipe-step-meta' }, meta.filter(Boolean)),
          run ? el('p', { class: 'pipe-step-state' }, [STEP_LABEL[status] || status]) : null,
          status === 'failed' && s.error ? el('p', { class: 'pipe-step-error' }, [stepError(s)]) : null,
          status === 'done' && s.note ? el('p', { class: 'pipe-step-note' }, [s.note]) : null,
          status === 'running' ? el('div', { class: 'bar indet' }, [el('i')]) : null,
          status === 'failed' && onRetry ? failedActions(s, i, onRetry) : null,
        ]),
        thumb,
      ]);
      list.append(row);
    });
    return list;
  }

  function renderPipeline() {
    const v = pipe.view;
    els.pipeBody.textContent = '';
    if (!v) return;
    if (v.run) renderRunView(v);
    else renderSetupForm(v);
  }

  function renderSetupForm(v) {
    const p = v.pipeline;
    els.pipelineTitle.textContent = 'Pipeline';
    const subject = el('textarea', { id: 'pipeSubject', maxlength: '60', rows: '2', placeholder: p.subject_hint, spellcheck: 'false' }, [pipe.subject]);
    subject.addEventListener('input', () => { pipe.subject = subject.value; });
    const tag = el('input', { id: 'pipeTag', maxlength: '32', spellcheck: 'false', value: pipe.tag });
    tag.addEventListener('input', () => { pipe.tag = tag.value; });
    const form = el('div', { class: 'panel-box pipe-form' }, [
      el('label', { class: 'field' }, [el('span', { class: 'k' }, [p.element_kind === 'product' ? 'The product, in a few words' : p.needs_image ? "What's in it, in a few words" : 'Who it is, in a few words']), subject]),
      el('label', { class: 'field' }, [el('span', { class: 'k' }, ['Element tag']), tag]),
      el('p', { class: 'muted small' }, ['These words and the tag become the run\'s Element, so later steps keep the same look.']),
    ]);
    if (p.needs_image) {
      const pick = el('button', { type: 'button', class: 'small-btn' }, [pipe.image ? 'Change photo' : 'Add a photo']);
      pick.addEventListener('click', () => els.pipeFile.click());
      form.prepend(el('div', { class: 'pipe-photo' }, [
        pipe.image ? el('img', { src: pipe.image, alt: 'Start photo' }) : el('div', { class: 'pipe-photo-empty', 'aria-hidden': 'true' }, ['+']),
        el('div', { class: 'pipe-photo-text' }, [el('span', { class: 'k' }, ['Start photo']), el('p', { class: 'muted small' }, [p.image_hint]), pick]),
      ]));
    }
    const t = p.total || {};
    const missing = (p.keys || []).filter((k) => !k.set).map((k) => k.env);
    const first = p.steps[0];
    const go = actionButton(first.paid ? first : null, first.paid ? `start with ${first.title}` : 'start', (b) => startRun(p, b), 'pipeStart');
    const settingsBtn = el('button', { type: 'button', class: 'small-btn' }, ['Open Settings']);
    settingsBtn.addEventListener('click', () => openSettings(missing[0]));
    appendAll(els.pipeBody, [
      el('div', { class: 'pipe-head' }, [el('h3', {}, [p.title]), el('p', { class: 'muted' }, [p.blurb])]),
      form,
      stepList(p.steps, null),
      p.note ? el('p', { class: 'pipe-note' }, [p.note]) : null,
      missing.length ? el('div', { class: 'pipe-warn' }, [el('p', {}, [`Needs ${missing.join(', ')} in Settings before its paid steps can run.`]), settingsBtn]) : null,
      el('div', { class: 'pipe-dock' }, [el('div', { class: 'pipe-total' }, [
        el('span', { class: 'k' }, ['Full list price']),
        el('strong', {}, [totalText(t)]),
        el('p', { class: 'muted small' }, [`On your own keys, at the providers' list prices (rounded up per step). ${t.paid_steps} step${t.paid_steps === 1 ? '' : 's'} cost money, and each one waits for your click. Free steps run on their own.`]),
      ]),
      el('div', { class: 'panel-actions' }, [go, el('span', { class: 'key-note', id: 'pipeNote', role: 'status' }, [v.note || ''])])]),
    ]);
  }

  async function uploadPipePhoto(file) {
    if (!file || !pipe.view) return;
    pipe.view.note = 'Uploading...';
    renderPipeline();
    try {
      if (!/^image\/(png|jpeg|webp)$/.test(file.type)) throw new Error('Use a PNG, JPEG or WebP photo.');
      if (file.size > MAX_UPLOAD) throw new Error('That file is too large (40 MB max).');
      pipe.image = (await uploadData(await fileToDataUrl(file))).url;
      pipe.view.note = '';
    } catch (e) {
      pipe.view.note = e.message || 'Upload failed.';
    }
    renderPipeline();
  }

  const CREATE_ERRORS = {
    image_required: 'Add the start photo first.',
    invalid_subject: 'Say who or what it is in a few words (up to 60 characters).',
    invalid_tag: 'Tag: letters, numbers, - or _, starting with a letter or number.',
    tag_in_use: 'That @tag is already one of your Elements. Pick another tag.',
    tag_in_run: 'Another unfinished run uses that @tag. Pick another tag, or finish or forget that run.',
    invalid_image: 'That photo is missing. Add it again.',
  };

  async function startRun(p, btn) {
    if (pipe.busy) return;
    pipe.busy = true;
    btn.disabled = true;
    const note = (t) => { pipe.view.note = t; const n = $('#pipeNote'); if (n) n.textContent = t; };
    note('Starting...');
    try {
      const body = { pipeline: p.id, subject: pipe.subject.trim(), tag: pipe.tag.trim(), image: pipe.image };
      const r = await call('/api/pipelines/runs', { method: 'POST', body });
      if (r.status !== 201 || !r.data || !r.data.run) {
        const code = r.data && r.data.error;
        note(CREATE_ERRORS[code] || (code ? `The engine refused this (${code}).` : `Could not start (${r.status}).`));
        return;
      }
      let run = r.data.run;
      // The Start button is the click for a paid first step.
      if (run.status === 'waiting' && run.current === 0) {
        const s = await call(`/api/pipelines/runs/${encodeURIComponent(run.id)}/step`, { method: 'POST', body: { step: 0 } });
        if (s.data && s.data.run) run = s.data.run;
      }
      pipe.runs = [run, ...pipe.runs.filter((x) => x.id !== run.id)];
      pipe.view = { pipeline: p, run, note: '' };
      renderPipeline();
      refreshJobs();
      loadRuns();
    } catch (_) {
      note('Could not reach the engine.');
      setOffline(true);
    } finally {
      pipe.busy = false;
      btn.disabled = false;
    }
  }

  async function clickStep(run, i, btn) {
    if (pipe.busy) return;
    pipe.busy = true;
    btn.disabled = true;
    try {
      const r = await call(`/api/pipelines/runs/${encodeURIComponent(run.id)}/step`, { method: 'POST', body: { step: i } });
      if (r.data && r.data.run) {
        pipe.view.run = r.data.run;
        pipe.runs = [r.data.run, ...pipe.runs.filter((x) => x.id !== run.id)];
      }
      pipe.view.note = r.ok ? '' : 'That step is not ready. The run was refreshed.';
      renderPipeline();
      refreshJobs();
      loadRuns();
    } catch (_) {
      pipe.view.note = 'Could not reach the engine.';
      setOffline(true);
      renderPipeline();
    } finally {
      pipe.busy = false;
    }
  }

  // Opens a job's card in the gallery (closing the panel) and points it out.
  function openInGallery(jobId) {
    closePanel();
    const entry = jobId && cards.get(jobId);
    if (!entry) return;
    entry.el.scrollIntoView({ block: 'center', behavior: reduceMotion ? 'auto' : 'smooth' });
    entry.el.classList.add('flash');
    setTimeout(() => entry.el.classList.remove('flash'), 1800);
  }

  function openEditPanel() {
    loadEditServer();
    openPanel(els.editor, () => { renderEditor(); requestAnimationFrame(() => { sizePreview(); updateHead(); }); });
  }

  // The finished run's results, large, with the one thing to do next.
  function finishedHero(run) {
    const made = run.steps.filter((s) => s.kind === 'generate' && s.status === 'done');
    const strip = el('div', { class: 'pipe-results' });
    const isClip = (s) => (s.outputs || []).some((x) => x.kind === 'video');
    // A run that ends on the Edit timeline shows its clips first (the finished film), then the pictures before them.
    const shown = run.steps.some((s) => s.kind === 'edit') ? [...made.filter(isClip), ...made.filter((s) => !isClip(s))] : made;
    for (const s of shown) {
      const o = (s.outputs || []).find((x) => x.kind === 'video') || (s.outputs || []).find((x) => x.kind === 'image');
      if (!o) continue;
      const media = o.kind === 'video' ? stillVideo(o.url, s.title) : el('img', { src: o.url, alt: s.title, decoding: 'async' });
      const fig = el('figure', o.kind === 'video' && !reduceMotion ? { tabindex: '0' } : {}, [media, el('figcaption', {}, [s.title])]);
      if (o.kind === 'video') playOnHover(fig, media);
      strip.append(fig);
    }
    // A fade on the right edge while more results are off to the side.
    const edge = () => strip.classList.toggle('more', strip.scrollLeft + strip.clientWidth < strip.scrollWidth - 4);
    strip.addEventListener('scroll', edge, { passive: true });
    requestAnimationFrame(edge);
    strip.querySelectorAll('img, video').forEach((m) => m.addEventListener(m.tagName === 'IMG' ? 'load' : 'loadeddata', edge, { once: true }));
    const edit = run.steps.find((s) => s.kind === 'edit');
    const model = [...run.steps].reverse().find((s) => s.kind === 'generate' && /3D/i.test(s.title));
    const last = [...made].reverse()[0];
    const main = edit
      ? el('button', { type: 'button', class: 'pill quiet', id: 'pipeMain' }, ['Open in Edit'])
      : el('button', { type: 'button', class: 'pill quiet', id: 'pipeMain' }, [model ? 'Open 3D model' : 'Open in gallery']);
    main.addEventListener('click', () => (edit ? openEditPanel() : openInGallery((model || last || {}).job)));
    const useTag = el('button', { type: 'button', class: 'small-btn' }, [`Use @${run.tag}`]);
    useTag.addEventListener('click', async () => {
      closePanel();
      await loadElements(); // the run saved this Element on the engine
      insertTag(run.tag);
      setStatus(`@${run.tag} is in the prompt: its picture goes with the request.`);
    });
    return el('section', { class: 'pipe-finished' }, [strip, el('div', { class: 'pipe-finished-acts' }, [main, useTag])]);
  }

  function renderRunView(v) {
    const run = v.run;
    const steps = run.steps;
    els.pipelineTitle.textContent = 'Pipeline run';
    const cur = steps[run.current];
    const retry = (i, b) => clickStep(run, i, b);
    const actions = [];
    if (cur && run.status === 'waiting' && !run.removed) actions.push(actionButton(cur, `run ${cur.title}`, (b) => clickStep(run, run.current, b), 'pipeGo'));
    const back = el('button', { type: 'button', class: 'link-btn plain' }, ['All pipelines']);
    back.addEventListener('click', () => { closePanel(true); openPanel(els.prompts, () => { loadLibrary(); loadPipelines(); }); });
    const forget = el('button', { type: 'button', class: 'link-btn' }, ['Forget this run']);
    forget.hidden = run.status === 'running';
    forget.addEventListener('click', async () => {
      const r = await call(`/api/pipelines/runs/${encodeURIComponent(run.id)}/delete`, { method: 'POST', body: {} });
      if (r.ok) {
        pipe.runs = pipe.runs.filter((x) => x.id !== run.id);
        pipe.view = null;
        renderRunChip();
        closePanel();
      }
    });
    const lead = run.removed ? 'This pipeline is no longer in the studio, so this run cannot go on. Its files are below and in the gallery.'
      : run.status === 'waiting'
      ? `Step ${run.current + 1} costs money, so it waits for you. Earlier results are below.`
      : run.status === 'failed' ? `Step ${run.current + 1} failed. Run again starts at that step and keeps everything before it.`
      : run.status === 'running' ? 'Working. You can close this panel or reload the page; the run keeps going.'
      : 'All steps are done. The files are also in the gallery.';
    appendAll(els.pipeBody, [
      el('div', { class: 'pipe-head' }, [
        el('div', { class: 'pipe-head-top' }, [el('h3', {}, [run.title || (run.removed ? 'Removed pipeline' : run.pipeline)]), el('span', { class: 'run-pill', 'data-state': run.status }, [RUN_LABEL[run.status] || run.status])]),
        el('p', { class: 'muted' }, [`${run.subject} · Element @${run.tag}`]),
      ]),
      run.status === 'done' && !run.removed ? finishedHero(run) : el('p', { class: 'pipe-lead', role: 'status' }, [lead]),
      stepList(steps, run, run.status === 'failed' && !run.removed ? retry : null),
      el('div', { class: 'pipe-dock' }, [
        run.removed ? null : el('div', { class: 'pipe-total small-total' }, [el('span', { class: 'k' }, ['Full list price']), el('strong', {}, [totalText(run.total)])]),
        actions.length || v.note ? el('div', { class: 'panel-actions' }, [...actions, el('span', { class: 'key-note', role: 'status' }, [v.note || ''])]) : null,
        el('div', { class: 'pipe-foot' }, [back, forget]),
      ]),
    ]);
  }

  // ---------- project look ----------

  async function loadProject() {
    try {
      const r = await call('/api/project');
      if (r.ok && r.data && r.data.project) state.project = r.data.project;
    } catch (_) { /* offline */ }
    els.stylePrefix.value = state.project.style_prefix || '';
    els.stillFirst.checked = !!state.project.still_first;
  }

  async function saveProject(ev) {
    ev.preventDefault();
    const body = { style_prefix: els.stylePrefix.value, still_first: els.stillFirst.checked };
    els.projectNote.textContent = 'Saving...';
    const r = await call('/api/project', { method: 'POST', body });
    if (r.ok) {
      state.project = r.data.project;
      els.projectNote.textContent = 'Saved. It goes on every image and video prompt.';
    } else {
      els.projectNote.textContent = (r.data && r.data.error) || `Could not save (${r.status}).`;
    }
  }

  // ---------- settings ----------

  function docsLink(k) {
    if (!k.docs || !/^https:\/\//.test(k.docs)) return null;
    const who = k.provider || k.env;
    return el('a', { class: 'key-docs', href: k.docs, target: '_blank', rel: 'noopener noreferrer', title: `${who} docs`, 'aria-label': `${who} docs` }, ['Docs']);
  }

  function renderKeys(focusEnv) {
    els.keyList.textContent = '';
    if (!state.keys.length) {
      els.keyList.append(el('p', { class: 'muted' }, ['The engine lists no key slots.']));
      return;
    }
    for (const k of state.keys) {
      const input = el('input', {
        type: 'password', name: 'value', autocomplete: 'new-password', spellcheck: 'false',
        'aria-label': `${k.env} value`, placeholder: 'Paste key',
      });
      const note = el('p', { class: 'key-note', role: 'status' });
      const btn = el('button', { type: 'submit' }, ['Save']);
      const light = el('span', { class: 'light' + (k.set ? ' set' : ''), 'aria-hidden': 'true' });
      const stateEl = el('span', { class: 'key-state' }, [k.set ? 'Set' : 'Not set']);
      const head = el('div', { class: 'key-head' }, [
        light,
        el('span', { class: 'key-env' }, [k.env]),
        stateEl,
        docsLink(k),
      ]);
      const form = el('form', { class: 'key-form', autocomplete: 'off' }, [input, btn, note]);
      form.addEventListener('submit', (ev) => saveKey(ev, k, { input, note, btn, light, stateEl }));
      els.keyList.append(el('div', { class: 'key', 'data-env': k.env }, [head, form]));
    }
    focusKey(focusEnv);
  }

  function focusKey(env) {
    const row = env ? els.keyList.querySelector(`[data-env="${CSS.escape(env)}"]`) : null;
    if (!row) {
      els.closeSettings.focus();
      return;
    }
    row.classList.add('flash');
    setTimeout(() => row.classList.remove('flash'), 1800);
    row.scrollIntoView({ block: 'center', behavior: reduceMotion ? 'auto' : 'smooth' });
    row.querySelector('input').focus({ preventScroll: true });
  }

  async function saveKey(ev, k, ui) {
    ev.preventDefault();
    const value = ui.input.value;
    ui.input.value = '';
    if (!value.trim()) {
      ui.note.textContent = 'Enter a key first.';
      ui.input.focus();
      return;
    }
    ui.btn.disabled = true;
    ui.note.textContent = 'Saving...';
    try {
      const r = await call('/api/keys', { method: 'POST', body: { env: k.env, value } });
      if (r.ok) {
        k.set = true;
        ui.light.classList.add('set');
        ui.stateEl.textContent = 'Set';
        ui.note.textContent = 'Saved';
        await reloadModels();
        updateEmpty();
      } else {
        ui.input.value = value; // keep what they typed so they can fix it
        ui.note.textContent = (r.data && r.data.error) || `Could not save (${r.status}).`;
      }
    } catch (_) {
      ui.input.value = value;
      ui.note.textContent = 'Engine offline.';
      setOffline(true);
    } finally {
      ui.btn.disabled = false;
    }
  }

  async function loadKeys(focusEnv) {
    try {
      const r = await call('/api/keys');
      if (r.ok) state.keys = (r.data && r.data.keys) || [];
    } catch (_) {
      setOffline(true);
    }
    renderKeys(focusEnv);
    updateEmpty();
  }

  async function reloadModels() {
    try {
      const r = await call('/api/models');
      if (r.ok) {
        state.models = (r.data && r.data.models) || [];
        renderModels();
      }
    } catch (_) {
      setOffline(true);
    }
  }

  let openPanelEl = null;

  // Slide-over panels (Settings, Elements, Edit). One at a time; the page behind is inert.
  function openPanel(panel, onOpen) {
    if (openPanelEl && openPanelEl !== panel) closePanel(true);
    if (!panel.classList.contains('open')) lastFocus = document.activeElement;
    panel.classList.add('open');
    panel.inert = false;
    els.shell.inert = true;
    els.scrim.hidden = false;
    openPanelEl = panel;
    document.querySelectorAll(`[aria-controls="${panel.id}"]`).forEach((b) => b.setAttribute('aria-expanded', 'true'));
    if (onOpen) onOpen();
  }

  function closePanel(keepFocus) {
    const panel = openPanelEl;
    if (!panel) return;
    if (panel === els.editor) pause();
    panel.classList.remove('open');
    panel.inert = true;
    els.shell.inert = false;
    els.scrim.hidden = true;
    openPanelEl = null;
    document.querySelectorAll(`[aria-controls="${panel.id}"]`).forEach((b) => b.setAttribute('aria-expanded', 'false'));
    if (!keepFocus && lastFocus && lastFocus.focus) lastFocus.focus();
  }

  function openSettings(focusEnv) {
    openPanel(els.settings, () => {
      loadKeys(focusEnv);
      loadProject();
      loadSpending();
    });
  }

  // ---------- spending ----------
  // The last week of paid runs from the studio's own jobs: the estimate shown before Run, and what fal charged once
  // its billing reports the run (only with the read-only FAL_BILLING_KEY). "Check fal now" also shows fal's own
  // 30-day total, which counts everything on the fal account, not only this studio.

  const usd = (v) => (v > 0 && v < 0.01 ? '< $0.01' : `$${v.toFixed(2)}`);

  async function loadSpending(keepNote) {
    if (!keepNote) els.spendNote.textContent = '';
    let d = null;
    try {
      const r = await call('/api/spending');
      if (r.ok) d = r.data;
    } catch (_) {
      setOffline(true);
    }
    renderSpending(d);
  }

  function renderSpending(d) {
    els.spendRows.textContent = '';
    els.spendActions.hidden = !(d && d.billing_key && !d.test);
    if (!d) {
      els.spendIntro.textContent = 'Engine offline.';
      return;
    }
    els.spendIntro.textContent = d.test
      ? 'Test mode: nothing here is charged.'
      : d.billing_key
        ? 'What each run was estimated at, and what fal charged once its billing shows the run (some minutes after it ends).'
        : 'Estimates only. Save fal\u2019s read-only billing key (FAL_BILLING_KEY) below to see what fal actually charged.';
    const week = d.week || [];
    if (!week.length) {
      els.spendRows.append(el('p', { class: 'muted' }, ['No paid runs this week.']));
      return;
    }
    let est = 0, charged = 0, waiting = 0;
    for (const w of week) {
      est += w.estimate_usd; charged += w.charged_usd; waiting += w.uncharged;
      // Only fal runs can be billed by fal; another provider's run shows its estimate alone.
      const billed = w.billable - w.uncharged;
      const paid = !d.billing_key || !w.billable ? ''
        : billed ? `fal charged ${usd(w.charged_usd)}${w.uncharged ? ` for ${billed}` : ''}` : 'not billed yet';
      const estimate = w.unestimated === w.runs ? 'no estimate' : `est. ${usd(w.estimate_usd)}${w.unestimated ? ' +' : ''}`;
      els.spendRows.append(el('div', { class: 'spend-row' }, [
        el('span', { class: 'spend-model' }, [w.name ? `${w.name} \u00b7 Playground` : modelName(w.model)]),
        el('span', { class: 'spend-runs' }, [`${w.runs} ${w.runs === 1 ? 'run' : 'runs'}`]),
        el('span', { class: 'spend-est' }, [estimate]),
        paid ? el('span', { class: 'spend-paid' }, [paid]) : null,
      ]));
    }
    const total = [`Estimated ${usd(est)}`];
    if (d.billing_key) total.push(`fal charged ${usd(charged)}${waiting ? ` (${waiting} not billed yet)` : ''}`);
    els.spendRows.append(el('p', { class: 'spend-total' }, [total.join(' \u00b7 ')]));
  }

  async function refreshSpending() {
    els.spendRefresh.disabled = true;
    els.spendNote.textContent = 'Asking fal\u2026';
    try {
      const r = await call('/api/spending/refresh', { method: 'POST', body: {} });
      const d = r.data || {};
      if (!r.ok) {
        // A stale launch cookie already shows "Reload the page".
        els.spendNote.textContent = d.error === 'launch_token' ? '' : `Could not ask fal (${r.status}).`;
        return;
      }
      const parts = [];
      if (d.error === 'key_refused') parts.push('fal refused the billing key: save a key made with fal\u2019s Billing preset.');
      else if (!d.ok) parts.push(`${d.found ? `${d.found} ${d.found === 1 ? 'charge' : 'charges'} found, but ` : ''}fal\u2019s billing did not answer; try again in a minute.`);
      else parts.push(d.found ? `${d.found} ${d.found === 1 ? 'charge' : 'charges'} found.` : 'No new charges.');
      if (d.fal && typeof d.fal.total_usd === 'number') {
        const top = (d.fal.by_endpoint || []).slice(0, 3).map((e) => `${e.endpoint} ${usd(e.usd)}`).join(', ');
        parts.push(`Your whole fal account, last 30 days: ${usd(d.fal.total_usd)}${top ? ` (${top})` : ''}.`);
      }
      els.spendNote.textContent = parts.join(' ');
      await loadSpending(true);
    } catch (_) {
      els.spendNote.textContent = 'Engine offline.';
      setOffline(true);
    } finally {
      els.spendRefresh.disabled = false;
    }
  }

  function closeSettings() {
    if (openPanelEl === els.settings) closePanel();
  }

  // ---------- first setup ----------
  // A sheet over the studio on first launch: a provider key, the on-device speech model, then a
  // summary. Each step can be skipped; Settings > Setup opens it again.

  const SETUP_SEEN = 'agave.setup.v1';
  const setup = { step: 1, data: null, keys: [], timer: null, samples: [], prevFocus: null };
  const sEl = {
    root: $('#setup'), stepNo: $('#setupStepNo'), keyForm: $('#setupKeyForm'), key: $('#setupKey'),
    keyNote: $('#setupKeyNote'), keyTag: $('#setupKeyTag'), keySave: $('#setupKeySave'), models: $('#setupModels'),
    modelActions: $('#setupModelActions'), skipModel: $('#setupSkipModel'), ready: $('#setupReady'), done: $('#setupDone'),
    summary: $('#setupSummary'),
  };
  const MB = (n) => Math.round(n / 1e6);

  function setupSeen() {
    try { return localStorage.getItem(SETUP_SEEN) === 'done'; } catch (_) { return false; }
  }
  function markSetupSeen() {
    try { localStorage.setItem(SETUP_SEEN, 'done'); } catch (_) { /* private mode: it shows again next time */ }
  }

  async function loadSetup() {
    const [a, k] = await Promise.all([call('/api/setup'), call('/api/keys')]);
    if (a.ok && a.data) setup.data = a.data;
    if (k.ok && k.data) setup.keys = k.data.keys || [];
    return setup.data;
  }

  const keySet = (env) => setup.keys.some((k) => k.env === env && k.set);
  const busy = (p) => ['downloading', 'checking', 'preparing'].includes(p.state);

  function setupStep(n) {
    setup.step = n;
    sEl.root.querySelectorAll('[data-setup-step]').forEach((sec) => { sec.hidden = Number(sec.dataset.setupStep) !== n; });
    sEl.root.querySelectorAll('[data-bar]').forEach((b) => {
      const i = Number(b.dataset.bar);
      b.className = i < n ? 'done' : i === n ? 'on' : '';
    });
    sEl.stepNo.textContent = `Step ${n} of 3`;
    if (n === 1) renderSetupKey();
    if (n === 2) renderSetupModels();
    if (n === 3) renderSetupReady();
    const first = sEl.root.querySelector(`[data-setup-step="${n}"] input, [data-setup-step="${n}"] .pill`);
    if (first) first.focus({ preventScroll: true });
  }

  async function openSetup(step) {
    setup.prevFocus = document.activeElement;
    if (openPanelEl) closePanel(true);
    await loadSetup();
    sEl.root.hidden = false;
    els.shell.inert = true;
    setupStep(step || 1);
    pollSetup();
  }

  function closeSetup() {
    if (sEl.root.hidden) return;
    sEl.root.hidden = true;
    els.shell.inert = false;
    markSetupSeen();
    renderSetupSummary();
    if (setup.prevFocus && setup.prevFocus.focus) setup.prevFocus.focus();
  }

  function renderSetupKey() {
    const set = keySet('FAL_KEY');
    sEl.keyTag.textContent = set ? 'Saved' : 'Start here';
    sEl.keyTag.className = 'setup-tag ' + (set ? 'ok' : 'rec');
    sEl.key.placeholder = set ? 'Saved. Paste to replace it' : 'Paste your fal.ai key';
    sEl.keySave.lastChild.textContent = set ? 'Continue' : 'Save and continue';
  }

  async function saveSetupKey(ev) {
    ev.preventDefault();
    const value = sEl.key.value.trim();
    sEl.key.value = '';
    if (!value) {
      if (keySet('FAL_KEY')) return setupStep(2);
      sEl.keyNote.textContent = 'Paste a key first, or skip for now.';
      sEl.key.focus();
      return;
    }
    sEl.keySave.disabled = true;
    sEl.keyNote.textContent = 'Saving...';
    try {
      const r = await call('/api/keys', { method: 'POST', body: { env: 'FAL_KEY', value } });
      if (!r.ok) {
        sEl.keyNote.textContent = 'That key was not saved. Check it and try again.';
        return;
      }
      sEl.keyNote.textContent = '';
      await loadSetup();
      refreshFlagsAfterKey();
      setupStep(2);
    } catch (_) {
      sEl.keyNote.textContent = 'The engine is not answering. Is it running?';
    } finally {
      sEl.keySave.disabled = false;
    }
  }

  // The rest of the app learns about the new key (without moving focus into Settings).
  function refreshFlagsAfterKey() {
    state.keys = setup.keys;
    updateEmpty();
  }

  function setupError(e) {
    const m = String(e || '');
    if (m.startsWith('not_enough_space')) return `Not enough free space on this computer (${m.split(': ')[1] || ''}). Free some space, then retry.`;
    if (m.startsWith('file_check_failed')) return 'A file did not match its published fingerprint and was deleted. Retry downloads it again.';
    if (m.startsWith('download_failed')) return `The download server refused (${m.split(': ')[1] || 'error'}). Try again later.`;
    if (m === 'connection_lost') return 'Connection lost. Retry picks up where it stopped.';
    return 'The download stopped. Retry picks up where it stopped.';
  }

  // Download speed from the last few seconds of polls.
  function rate(p) {
    const now = performance.now();
    setup.samples.push([now, p.done]);
    setup.samples = setup.samples.filter(([t]) => now - t < 4000);
    const [t0, d0] = setup.samples[0];
    return now - t0 > 900 ? (p.done - d0) / ((now - t0) / 1000) : 0;
  }

  function modelCard(m, main) {
    const p = m.progress;
    const card = el('div', { class: 'setup-card' });
    const tag = p.state === 'ready' ? 'Ready' : busy(p) || p.state === 'paused' ? `${Math.floor((100 * p.done) / Math.max(1, p.total))}%` : `${MB(m.size)} MB`;
    card.append(el('div', { class: 'setup-card-top' }, [
      el('div', {}, [el('div', { class: 'setup-name' }, [m.name]), el('div', { class: 'muted small' }, [m.source])]),
      el('span', { class: 'setup-tag' + (p.state === 'ready' ? ' ok' : main ? ' rec' : '') }, [tag]),
    ]));
    const facts = [m.languages, m.engine === 'parakeet' ? 'runs on the Neural Engine' : 'runs on the CPU'];
    if (p.state !== 'ready') facts.push(`needs ${Math.ceil((m.size + 104857600) / 1e8) / 10} GB free`);
    card.append(el('div', { class: 'setup-facts' }, facts.map((f) => el('span', {}, [f]))));
    if (p.state !== 'idle') {
      const cls = p.state === 'ready' ? 'done' : p.state === 'failed' ? 'fail' : p.state === 'checking' || p.state === 'preparing' ? 'check' : '';
      const bar = el('div', { class: 'setup-bar ' + cls }, [el('i')]);
      bar.firstChild.style.width = `${p.state === 'ready' || p.state === 'checking' || p.state === 'preparing' ? 100 : (100 * p.done) / Math.max(1, p.total)}%`;
      card.append(bar);
      let left = '';
      let right = '';
      if (p.state === 'downloading') {
        const r = rate(p);
        left = `${MB(p.done)} MB of ${MB(p.total)} MB`;
        right = r > 0 ? `${(r / 1e6).toFixed(1)} MB/s · about ${Math.max(1, Math.round((p.total - p.done) / r))} s left` : 'Starting...';
      } else if (p.state === 'checking') {
        left = 'Checking every file against its published fingerprint';
      } else if (p.state === 'preparing') {
        left = 'Preparing the model for this computer (about half a minute)';
      } else if (p.state === 'ready') {
        left = el('span', { class: 'ok' }, ['Installed and checked']);
      } else if (p.state === 'failed') {
        left = el('span', { class: 'err' }, [setupError(p.error)]);
      } else if (p.state === 'paused') {
        left = `Paused at ${MB(p.done)} MB of ${MB(p.total)} MB`;
        right = 'Resume picks up from here';
      }
      card.append(el('div', { class: 'setup-status', role: 'status' }, [el('span', {}, [left]), el('span', {}, [right])]));
    }
    return card;
  }

  function setupButton(label, cls, fn, dot) {
    const b = el('button', { type: 'button', class: cls }, [dot ? el('span', { class: 'dot' }) : null, label]);
    b.addEventListener('click', fn);
    return b;
  }

  async function startModel(id) {
    setup.samples = [];
    const r = await call('/api/setup/download', { method: 'POST', body: { model: id } });
    if (!r.ok) {
      sEl.models.prepend(el('p', { class: 'key-note' }, [r.data && r.data.error === 'speech_tool_missing'
        ? 'The speech tool is not built on this Mac (native/speech/build.sh).' : 'The download could not start.']));
      return;
    }
    await loadSetup();
    renderSetupModels();
    pollSetup();
  }

  function renderSetupModels() {
    const d = setup.data;
    sEl.models.textContent = '';
    sEl.modelActions.textContent = '';
    if (!d) {
      sEl.models.append(el('p', { class: 'key-note' }, ['The engine is not answering. Is it running?']));
      return;
    }
    const byId = Object.fromEntries(d.models.map((m) => [m.id, m]));
    const parakeet = byId['parakeet-v3'];
    const whisper = byId['whisper-small'];
    // Parakeet is the default; Whisper is for languages Parakeet lacks, or when its tool is missing.
    const main = d.speech.parakeet_tool ? parakeet : whisper;
    const alt = main === parakeet ? whisper : null;
    sEl.models.append(modelCard(main, true));
    // Whisper runs through whisper.cpp's own tool, which Agave does not ship.
    if (main === whisper && !d.speech.whisper_tool) {
      sEl.models.append(el('p', { class: 'key-note' }, ['Whisper also needs whisper.cpp: put ', el('code', {}, ['whisper-cli']),
        ' on your PATH (on a Mac, ', el('code', {}, ['brew install whisper-cpp']), '), then reopen setup.']));
    }
    if (alt) {
      if (!['idle'].includes(alt.progress.state)) sEl.models.append(modelCard(alt, false));
      else {
        sEl.models.append(el('div', { class: 'setup-alt' }, [
          el('span', {}, [`Need Japanese, Chinese, Korean, Hindi, Arabic or another language? ${alt.name} covers ${alt.languages} (${MB(alt.size)} MB).`]),
          setupButton(`Get ${alt.name}`, 'setup-ghost sm', () => startModel(alt.id)),
        ]));
      }
    }
    const p = main.progress;
    const anyReady = d.models.some((m) => m.progress.state === 'ready');
    sEl.skipModel.hidden = anyReady;
    $('#setupModelNote').hidden = anyReady && !d.models.some((m) => busy(m.progress));
    if (p.state === 'idle') sEl.modelActions.append(setupButton(`Download ${MB(main.size)} MB`, 'pill', () => startModel(main.id), true));
    else if (p.state === 'failed') sEl.modelActions.append(setupButton('Retry', 'pill', () => startModel(main.id), true));
    else if (p.state === 'paused') sEl.modelActions.append(setupButton(`Resume (${MB(p.total - p.done)} MB left)`, 'pill', () => startModel(main.id), true));
    else if (p.state === 'downloading') {
      sEl.modelActions.append(setupButton('Cancel', 'setup-ghost', async () => {
        await call('/api/setup/cancel', { method: 'POST', body: { model: main.id } });
        setTimeout(async () => { await loadSetup(); renderSetupModels(); }, 300);
      }));
    }
    if (busy(p)) sEl.modelActions.append(setupButton('Continue while it downloads', 'pill', () => setupStep(3), true));
    if (p.state === 'ready' || (anyReady && !busy(p))) sEl.modelActions.append(setupButton('Continue', 'pill', () => setupStep(3), true));
  }

  function renderSetupReady() {
    const d = setup.data;
    const rows = [];
    const row = (ok, name, note, tag) => rows.push(el('div', {}, [
      el('span', { class: 'setup-tick' + (ok ? ' y' : '') }, [ok ? '✓' : '–']),
      el('span', {}, [name, el('span', { class: 'muted small' }, [` · ${note}`])]),
      el('span', { class: 'setup-tag' + (ok ? ' ok' : '') }, [tag]),
    ]));
    row(keySet('FAL_KEY'), 'fal.ai key', 'images and video', keySet('FAL_KEY') ? 'Ready' : 'Later');
    const engine = d && d.speech.engine;
    const dl = d && d.models.find((m) => busy(m.progress));
    const whisperNoTool = d && !d.speech.whisper_tool && d.models.some((m) => m.engine === 'whisper' && m.progress.state === 'ready');
    row(!!engine, 'Captions', engine === 'parakeet' ? 'Parakeet v3, on device' : engine === 'whisper' ? 'Whisper small, on device' : dl ? `${dl.name} still downloading`
      : whisperNoTool ? 'Whisper needs whisper-cli on your PATH' : 'needs the speech model',
      engine ? 'Ready' : dl ? 'Downloading' : 'Later');
    row(keySet('MESHY_API_KEY'), '3D from photos', keySet('MESHY_API_KEY') ? 'Meshy key saved' : 'needs a Meshy key', keySet('MESHY_API_KEY') ? 'Ready' : 'Later');
    sEl.ready.replaceChildren(...rows);
  }

  async function renderSetupSummary() {
    if (!sEl.summary) return;
    const d = setup.data || (await loadSetup());
    if (!d) return;
    const e = d.speech.engine;
    sEl.summary.textContent = `Key: ${keySet('FAL_KEY') ? 'fal.ai saved' : 'none yet'} · Captions: ${e === 'parakeet' ? 'Parakeet v3' : e === 'whisper' ? 'Whisper small' : 'not set up'}`;
  }

  // While a model is busy, refresh its card twice a second (only while the sheet is open).
  function pollSetup() {
    clearTimeout(setup.timer);
    if (sEl.root.hidden || !setup.data || !setup.data.models.some((m) => busy(m.progress))) return;
    setup.timer = setTimeout(async () => {
      await loadSetup();
      if (setup.step === 2) renderSetupModels();
      if (setup.step === 3) renderSetupReady();
      pollSetup();
    }, 500);
  }

  async function maybeFirstSetup() {
    if (setupSeen()) return;
    const d = await loadSetup().catch(() => null);
    if (!d) return;
    if (!keySet('FAL_KEY') || !d.speech.engine) openSetup(keySet('FAL_KEY') ? 2 : 1);
  }

  function bindSetup() {
    sEl.keyForm.addEventListener('submit', saveSetupKey);
    sEl.root.querySelectorAll('[data-setup-next]').forEach((b) => b.addEventListener('click', () => setupStep(Number(b.dataset.setupNext))));
    sEl.done.addEventListener('click', closeSetup);
    $('#openSetup').addEventListener('click', () => openSetup(1));
    sEl.root.addEventListener('keydown', (e) => { if (e.key === 'Escape') { e.stopPropagation(); closeSetup(); } });
  }

  // ---------- wiring ----------

  function bind() {
    document.querySelectorAll('.mode[data-mode]').forEach((b) =>
      b.addEventListener('click', () => { state.libModel = null; setMode(b.dataset.mode); }));
    els.boardBtn.addEventListener('click', showBoard);
    document.querySelectorAll('[data-open-settings]').forEach((b) =>
      b.addEventListener('click', () => openSettings()));
    els.closeSettings.addEventListener('click', closeSettings);
    els.spendRefresh.addEventListener('click', refreshSpending);
    els.scrim.addEventListener('click', () => closePanel());
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') closePanel();
    });
    document.querySelectorAll('[data-close-panel]').forEach((b) => b.addEventListener('click', () => closePanel()));
    document.querySelectorAll('[data-open-panel]').forEach((b) => b.addEventListener('click', () => {
      if (b.dataset.openPanel === 'library') openPanel(els.library, () => { renderNewElementImages(); loadElements(); });
      else if (b.dataset.openPanel === 'prompts') openPanel(els.prompts, () => { loadLibrary(); loadPipelines(); });
      else openPanel(els.editor, () => { renderEditor(); requestAnimationFrame(() => { sizePreview(); updateHead(); }); });
    }));
    els.addRef.addEventListener('click', () => els.fileIn.click());
    els.fileIn.addEventListener('change', () => { addFiles(els.fileIn.files); els.fileIn.value = ''; });
    els.form.addEventListener('dragover', (e) => { e.preventDefault(); els.form.classList.add('drop'); });
    els.form.addEventListener('dragleave', () => els.form.classList.remove('drop'));
    els.form.addEventListener('drop', (e) => {
      els.form.classList.remove('drop');
      if (!e.dataTransfer || !e.dataTransfer.files.length) return;
      e.preventDefault();
      addFiles(e.dataTransfer.files);
    });
    els.prompt.addEventListener('paste', (e) => {
      const files = e.clipboardData ? [...e.clipboardData.files] : [];
      if (files.some((f) => f.type.startsWith('image/'))) {
        e.preventDefault();
        addFiles(files);
      }
    });
    els.prompt.addEventListener('input', syncRefMode);
    els.countSel.addEventListener('change', renderCost);
    els.style.addEventListener('change', () => { els.lookAsk.hidden = true; });
    els.camera.addEventListener('change', renderCost);
    els.ref.addEventListener('input', () => { els.lookAsk.hidden = true; });
    els.recipe.addEventListener('change', () => {
      if (els.recipe.value && els.recipe.value !== 'sheet' && !refActive()) setStatus('This recipe works from a reference image: add one.', 'warn');
    });
    els.retryAll.addEventListener('click', runFailedAgain);
    els.elForm.addEventListener('submit', saveElement);
    els.addSong.addEventListener('click', () => els.songIn.click());
    els.songIn.addEventListener('change', () => { pickSong(els.songIn.files[0]); els.songIn.value = ''; });
    els.clearSong.addEventListener('click', () => { state.song = null; if (pv.song) pv.song.pause(); renderSong(); saveEdit(); });
    els.keepSound.addEventListener('change', () => { saveEdit(); els.pvVideo.muted = !(els.keepSound.checked && !state.song); });
    els.joinBtn.addEventListener('click', joinClips);
    els.pvPlay.addEventListener('click', () => (pv.playing ? pause() : play()));
    els.splitBtn.addEventListener('click', splitAtPlayhead);
    els.pausesBtn.addEventListener('click', removePauses);
    els.undoBtn.addEventListener('click', undoEdit);
    els.fixForm.addEventListener('submit', addFix);
    els.cleanVoice.addEventListener('change', saveEdit);
    els.loudness.addEventListener('change', saveEdit);
    els.forYouTube.addEventListener('click', () => exportFor('youtube'));
    els.forVertical.addEventListener('click', () => exportFor('vertical'));
    els.addClipBtn.addEventListener('click', () => els.clipIn.click());
    els.clipIn.addEventListener('change', () => { uploadClip(els.clipIn.files[0]); els.clipIn.value = ''; });
    // Space plays or pauses and S splits, except while typing.
    els.editor.addEventListener('keydown', (e) => {
      if (e.target.closest('input, textarea, select') || e.metaKey || e.ctrlKey || e.altKey) return;
      if (e.key === ' ' && !e.target.closest('button')) { e.preventDefault(); if (pv.playing) pause(); else play(); }
      else if (e.key === 's' || e.key === 'S') { e.preventDefault(); splitAtPlayhead(); }
    });
    window.addEventListener('resize', () => { if (openPanelEl === els.editor) { updateHead(); sizePreview(); } });
    els.projectForm.addEventListener('submit', saveProject);
    els.pipeFile.addEventListener('change', () => { uploadPipePhoto(els.pipeFile.files[0]); els.pipeFile.value = ''; });

    els.form.addEventListener('submit', submit);
    els.prompt.addEventListener('input', autosize);
    els.prompt.addEventListener('keydown', (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault();
        els.form.requestSubmit();
      }
    });
    els.ref.addEventListener('input', syncRefMode);
    els.model.addEventListener('change', () => {
      state.libModel = null;
      els.lookAsk.hidden = true;
      renderChips();
      renderCost();
      writeHash();
    });
    window.addEventListener('hashchange', () => {
      const h = readHash();
      if (h.mode === 'board') { showBoard(); return; }
      if (!MODES[h.mode]) return;
      state.want = h.model;
      setMode(h.mode);
    });
    for (const c of CHIPS) {
      $('#' + c.key).addEventListener('change', (e) => {
        state.chips[c.key] = c.parse(e.target.value);
        if (c.key === 'audio') state.audioPicked = true;
        renderCost();
      });
    }

    if ('ResizeObserver' in window) new ResizeObserver(measureDock).observe(els.dock);
    else window.addEventListener('resize', measureDock);
  }

  async function init() {
    bind();
    measureDock();
    const h = readHash();
    state.want = h.model;
    setMode(MODES[h.mode] ? h.mode : 'image');
    if (h.mode === 'board') showBoard();
    updateEmpty();
    loadEdit();
    renderSong();
    renderEditor();
    renderCost();
    renderFlags();
    loadEditServer();
    loadElements();
    loadProject();
    refreshJobs();
    bindSetup();
    maybeFirstSetup();
    loadPipelines();
    call('/api/health').then((r) => { els.testBanner.hidden = !(r.ok && r.data && r.data.fake_providers); }).catch(() => {});
  }

  init();
})();
