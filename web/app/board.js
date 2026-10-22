/* Agave Studio board: the real Excalidraw editor (web/vendor/excalidraw, built by vendor-src/excalidraw), boards
   saved on the engine as data/boards/<name>.excalidraw (docs/API.md, "Boards"). Finished results go on the board as
   pictures; each frame on the board is a shot that can fill the Video form. Nothing here starts a paid run: Send to
   Video only fills the form, and the run still waits for Generate. Text is always set with textContent. */

const ASSET_PATH = '/vendor/excalidraw/';
// Pictures are shrunk before they go into a board, since a board file embeds them (the engine takes 25 MB a board).
const MAX_SIDE = 1600;
const MAX_PICTURE_CHARS = 2_500_000; // about 1.9 MB of picture once decoded
const MAX_BOARD_BYTES = 25 * 1024 * 1024;
const RESULT_MIME = 'application/x-agave-result';
const UNTITLED = '';

let X = null; // the Excalidraw module
let api = null; // Excalidraw's imperative API
let bridge = null; // the studio's helpers (app.js)
let ui = null;
let loading = null;
let shotTimer = null;

// The board on screen. `saved` is the scene version last saved or loaded, so a change shows as unsaved.
const cur = { name: UNTITLED, rev: null, saved: 0, version: 0, files: 0, savedFiles: 0 };
// Boards switched away from with unsaved changes, kept until they are saved: name ('' for a new one) -> scene.
const kept = new Map();
let list = []; // saved boards from the engine

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

const ERRORS = {
  invalid_name: 'Use 1 to 60 letters, digits, spaces, - _ ( ), starting with a letter or digit.',
  board_exists: 'A board with that name already exists. Pick another name.',
  board_too_large: 'This board is over 25 MB. Remove some pictures, then save again.',
  too_many_boards: 'There are 500 boards already. Delete one first.',
  invalid_board: 'That file is not an Excalidraw board.',
  not_found: 'That board is no longer saved.',
  launch_token: 'This page lost its link to the studio. Reload the page, then try again.',
};
const errText = (r, fallback) => (r && r.data && ERRORS[r.data.error]) || fallback;

function note(msg, tone) {
  ui.note.textContent = msg || '';
  ui.note.className = 'board-note' + (tone ? ' ' + tone : '');
}

const validName = (n) => /^[A-Za-z0-9][A-Za-z0-9 _()-]{0,59}$/.test(n) && !n.endsWith(' ');
const isDirty = () => cur.version !== cur.saved || cur.files !== cur.savedFiles;
const hasUnsaved = () => isDirty() || kept.size > 0;

// ---------- what a board may load ----------
// A board file can come from anyone. Excalidraw keeps any `dataURL` string and any web-embed element, and the page
// policy leaves pictures open on purpose (reference images), so a hostile board could make the studio fetch from any
// host. Only embedded pictures are kept, and embeds never show.

const SAFE_PICTURE = /^data:image\/(png|jpeg|webp|gif|svg\+xml);base64,/;
const EMBEDS = new Set(['iframe', 'embeddable']);
const safePicture = (f) => !!f && typeof f.dataURL === 'string' && SAFE_PICTURE.test(f.dataURL);

// Returns the elements and files a board may show, and how many of each it had to take out. An embed is marked
// deleted with its custom data dropped; a picture whose file is not embedded data is marked deleted too, so the
// file is never shown or saved again.
function scrub(elements, files) {
  const keep = {};
  let dropped = 0;
  for (const [id, f] of Object.entries(files || {})) {
    if (safePicture(f)) keep[id] = f;
    else dropped++;
  }
  const bad = new Set(Object.keys(files || {}).filter((id) => !keep[id]));
  let removed = 0;
  const out = (elements || []).map((e) => {
    if (!e || e.isDeleted) return e;
    if (EMBEDS.has(e.type) || (e.type === 'image' && bad.has(e.fileId))) {
      removed++;
      const { customData: _drop, ...rest } = e;
      return { ...rest, isDeleted: true, version: (e.version || 1) + 1, versionNonce: Math.floor(Math.random() * 2 ** 31) };
    }
    return e;
  });
  return { elements: out, files: keep, dropped, removed };
}

function scrubNote(r) {
  if (r.dropped || r.removed) note('Pictures from the internet and web embeds are not shown on a board; they were left out.', 'warn');
}

// An Excalidraw clipboard payload (copied elements) or board file as text, or null.
function sceneFromText(text) {
  if (typeof text !== 'string' || text.length > MAX_BOARD_BYTES || !/^\s*\{/.test(text)) return null;
  try {
    const v = JSON.parse(text);
    return v && (v.type === 'excalidraw' || v.type === 'excalidraw/clipboard') && Array.isArray(v.elements) ? v : null;
  } catch (_) {
    return null;
  }
}

// Text a paste or drop carries that would make the editor fetch from another host: a board or copied elements with
// files that are not embedded pictures or with web embeds, HTML with a picture from the web, or a link to elsewhere.
function reachesOut(dt) {
  for (const type of dt.types) {
    if (type === 'Files') continue;
    const text = dt.getData(type) || '';
    if (/html/.test(type) && /<img[^>]+src\s*=\s*["']?\s*(https?:|\/\/)/i.test(text)) return true;
    if (type === 'text/uri-list' && /^\s*(https?:)?\/\//i.test(text) && !text.trim().startsWith(location.origin)) return true;
    const v = sceneFromText(text);
    if (v) {
      const r = scrub(v.elements, v.files);
      if (r.dropped || r.removed) return true;
    }
  }
  return false;
}

// A paste on the board is checked before Excalidraw sees it (Excalidraw listens on the document).
function onPaste(e) {
  if (!ui || ui.canvas.offsetParent === null || !e.clipboardData) return;
  const t = e.target;
  if (t && t.closest && t.closest('input, textarea, select, [contenteditable="true"]')) return;
  if (!reachesOut(e.clipboardData)) return;
  e.preventDefault();
  e.stopImmediatePropagation();
  note('That paste had pictures from the internet or web embeds, which a board does not load. Paste or drop the picture files themselves.', 'warn');
}

// A board file dropped on the canvas opens as an untitled board, through the same checks as a saved one (Excalidraw
// would load it as it is). Picture files go on to Excalidraw, which embeds them.
function onFileDrop(e) {
  const dt = e.dataTransfer;
  const files = [...dt.files];
  const other = files.filter((f) => !/^image\/(png|jpeg|webp|gif|svg\+xml)$/.test(f.type));
  if (!other.length && !(files.length === 0 && reachesOut(dt))) return;
  e.preventDefault();
  e.stopPropagation();
  ui.canvas.classList.remove('drop');
  if (!other.length || other[0].size > MAX_BOARD_BYTES) {
    note(other.length ? ERRORS.board_too_large : 'Only picture files and boards can be dropped here; links to the web are not loaded.', 'warn');
    return;
  }
  other[0].text().then((text) => {
    const v = sceneFromText(text);
    if (!v || v.type !== 'excalidraw') {
      note('Only picture files and Excalidraw boards can be dropped here.', 'warn');
      return;
    }
    openDropped(v);
  }, () => note('That file could not be read.', 'err'));
}

// ---------- the editor ----------

function loadEditor() {
  if (loading) return loading;
  // Excalidraw resolves its font files against this, so it must be a whole URL.
  window.EXCALIDRAW_ASSET_PATH = new URL(ASSET_PATH, location.href).href;
  const css = el('link', { rel: 'stylesheet', href: `${ASSET_PATH}excalidraw.css` });
  document.head.append(css);
  loading = import(`${ASSET_PATH}excalidraw.js`).then((mod) => { X = mod; });
  loading.catch(() => { loading = null; css.remove(); });
  return loading;
}

function mountEditor() {
  const h = X.React.createElement;
  const menu = h(X.MainMenu, null,
    h(X.MainMenu.DefaultItems.Export),
    h(X.MainMenu.DefaultItems.SaveAsImage),
    h(X.MainMenu.DefaultItems.SearchMenu),
    h(X.MainMenu.DefaultItems.Help),
    h(X.MainMenu.DefaultItems.ClearCanvas),
    h(X.MainMenu.Separator),
    h(X.MainMenu.DefaultItems.ToggleTheme),
    h(X.MainMenu.DefaultItems.ChangeCanvasBackground));
  const welcome = h(X.WelcomeScreen, null,
    h(X.WelcomeScreen.Hints.MenuHint),
    h(X.WelcomeScreen.Hints.ToolbarHint),
    h(X.WelcomeScreen.Center, null,
      h(X.WelcomeScreen.Center.Heading, null, 'Sketch shots, drop results in, frame each shot.')));
  const root = X.createRoot(ui.canvas);
  root.render(h(X.Excalidraw, {
    excalidrawAPI: (a) => { api = a; },
    langCode: 'en',
    initialData: { elements: [], appState: { viewBackgroundColor: '#ffffff', theme: 'dark' } },
    // Opening and saving go through the studio's own buttons, so the editor's file menu items are off.
    UIOptions: { canvasActions: { loadScene: false, saveToActiveFile: false, export: { saveFileToDisk: true } } },
    onChange: changed,
    onLinkOpen: openLink,
    // No web embeds: the page policy blocks every frame anyway (frame-src 'none').
    validateEmbeddable: () => false,
  }, menu, welcome));
}

// A picture's link opens the result's file in a new tab, so the board itself is never navigated away from. Only the
// studio's own result files open: a link a board brought from elsewhere stays put.
function openLink(element, event) {
  const link = element && element.link;
  if (!link) return;
  event.preventDefault();
  if (/^\/outputs\/[A-Za-z0-9._-]+$/.test(link)) window.open(link, '_blank', 'noopener,noreferrer');
  else note('Only links to the studio\'s own results open from a board.', 'warn');
}

function changed(elements, _appState, files) {
  // A backstop for whatever reached the editor some other way (a library item, a paste the page did not see).
  const r = scrub(elements, files);
  if (r.removed) {
    api.updateScene({ elements: r.elements, captureUpdate: X.CaptureUpdateAction.NEVER });
    scrubNote(r);
    return;
  }
  cur.version = X.getSceneVersion(elements);
  cur.files = Object.keys(files || {}).length;
  renderTitle();
  clearTimeout(shotTimer);
  shotTimer = setTimeout(renderShots, 250);
}

function scene() {
  return { elements: api.getSceneElementsIncludingDeleted(), appState: api.getAppState(), files: api.getFiles() };
}

function showScene(s, name, rev, dirty) {
  const clean = scrub(s.elements, s.files);
  scrubNote(clean);
  const data = X.restore({ elements: clean.elements, appState: s.appState, files: clean.files }, null, null);
  const theme = api.getAppState().theme;
  api.resetScene();
  api.updateScene({
    elements: data.elements,
    appState: { theme, viewBackgroundColor: (s.appState && s.appState.viewBackgroundColor) || '#ffffff', name: name || 'Untitled board' },
    captureUpdate: X.CaptureUpdateAction.NEVER,
  });
  const files = Object.values(data.files || {});
  if (files.length) api.addFiles(files);
  api.history.clear();
  cur.name = name;
  cur.rev = rev;
  cur.version = X.getSceneVersion(api.getSceneElementsIncludingDeleted());
  cur.files = Object.keys(api.getFiles()).length;
  cur.saved = dirty ? -1 : cur.version;
  cur.savedFiles = cur.files;
  ui.name.value = name;
  requestAnimationFrame(() => { if (data.elements.length) api.scrollToContent(undefined, { fitToContent: true }); });
  renderTitle();
  renderList();
  renderShots();
}

// Puts the board on screen aside (unsaved changes and all) before another one opens.
function keepCurrent() {
  if (!isDirty()) {
    kept.delete(cur.name);
    return;
  }
  const s = scene();
  kept.set(cur.name, { elements: s.elements, appState: { viewBackgroundColor: s.appState.viewBackgroundColor }, files: s.files, rev: cur.rev });
}

// ---------- boards on the engine ----------

async function refreshList() {
  const r = await bridge.call('/api/boards');
  list = r.ok && r.data && Array.isArray(r.data.boards) ? r.data.boards : list;
  renderList();
}

function renderList() {
  const sel = ui.open;
  sel.textContent = '';
  sel.append(el('option', { value: '' }, [list.length || kept.size ? 'Open a board…' : 'No saved boards yet']));
  const names = new Set(list.map((b) => b.name));
  for (const b of list) {
    const label = kept.has(b.name) || (b.name === cur.name && isDirty()) ? `${b.name} (unsaved changes)` : b.name;
    sel.append(el('option', { value: 'b:' + b.name }, [label]));
  }
  if (kept.has(UNTITLED) && cur.name !== UNTITLED) sel.append(el('option', { value: 'new:' }, ['Untitled board (unsaved)']));
  for (const n of kept.keys()) {
    if (n !== UNTITLED && !names.has(n)) sel.append(el('option', { value: 'b:' + n }, [`${n} (unsaved, deleted on disk)`]));
  }
  sel.value = '';
  ui.rename.disabled = !cur.name;
  ui.del.disabled = !cur.name || !names.has(cur.name);
}

function renderTitle() {
  const label = cur.name || 'Untitled board';
  ui.state.textContent = isDirty() ? `${label} · unsaved changes` : cur.rev ? `${label} · saved` : label;
  ui.state.classList.toggle('dirty', isDirty());
}

async function openBoard(name) {
  if (name === cur.name && !kept.has(name)) return;
  keepCurrent();
  const k = kept.get(name);
  if (k) {
    showScene(k, name, k.rev, true);
    kept.delete(name);
    note(name ? `${name} has the changes you had not saved yet.` : 'Your unsaved board is back.');
    return;
  }
  note(`Opening ${name}…`);
  const r = await bridge.call(`/api/boards/load?name=${encodeURIComponent(name)}`);
  if (!r.ok || !r.data || !r.data.board) {
    note(errText(r, 'That board could not be opened.'), 'err');
    return;
  }
  note('');
  showScene(r.data.board, r.data.name, r.data.rev, false);
}

function openDropped(v) {
  keepCurrent();
  showScene({ elements: v.elements, appState: v.appState || {}, files: v.files || {} }, UNTITLED, null, true);
  if (!ui.note.classList.contains('warn')) note('The dropped board is open as an untitled board. Name it, then Save.');
}

function newBoard() {
  keepCurrent();
  showScene({ elements: [], appState: {}, files: {} }, UNTITLED, null, false);
  kept.delete(UNTITLED);
  ui.name.focus();
  note('A new board. Name it, then Save.');
}

function boardJson() {
  const s = scene();
  return JSON.parse(X.serializeAsJSON(s.elements, s.appState, s.files, 'local'));
}

// force: save over a board changed elsewhere. asNew: the board was renamed or deleted elsewhere; save it again.
async function save(force, asNew) {
  // One save at a time: Enter in the name box would otherwise send a second one while the first is on its way.
  if (ui.save.disabled) return;
  const name = ui.name.value.trim();
  if (!validName(name)) {
    note(name ? ERRORS.invalid_name : 'Name the board first.', 'warn');
    ui.name.focus();
    return;
  }
  const board = boardJson();
  const size = new Blob([JSON.stringify(board)]).size;
  if (size > MAX_BOARD_BYTES) {
    note(ERRORS.board_too_large, 'err');
    return;
  }
  // A new name saves a new board (and never replaces another); the board's own name saves over its last save,
  // unless that changed since it was opened.
  const same = name === cur.name && cur.rev && !asNew;
  const body = { name, board, create: !same };
  if (same && !force) body.rev = cur.rev;
  ui.save.disabled = true;
  note('Saving…');
  const r = await bridge.call('/api/boards/save', { method: 'POST', body });
  ui.save.disabled = false;
  if (r.status === 409 && r.data && r.data.error === 'board_changed') {
    note('This board was saved from another window since you opened it.', 'warn');
    const over = el('button', { type: 'button', class: 'act' }, ['Save over it']);
    over.addEventListener('click', () => save(true));
    ui.note.append(' ', over);
    return;
  }
  if (r.status === 404 && body.rev) {
    note('This board was renamed or deleted from another window since you opened it.', 'warn');
    const again = el('button', { type: 'button', class: 'act' }, [`Save it as ${name}`]);
    again.addEventListener('click', () => save(false, true));
    ui.note.append(' ', again);
    return;
  }
  if (!r.ok || !r.data || !r.data.board) {
    note(errText(r, 'The board could not be saved.'), 'err');
    return;
  }
  const wasNew = !same && cur.name !== name;
  if (wasNew && cur.name === UNTITLED) kept.delete(UNTITLED);
  cur.name = r.data.board.name;
  cur.rev = r.data.board.rev;
  cur.saved = cur.version;
  cur.savedFiles = cur.files;
  kept.delete(cur.name);
  api.updateScene({ appState: { name: cur.name }, captureUpdate: X.CaptureUpdateAction.NEVER });
  note(`Saved ${cur.name}.`, 'ok');
  renderTitle();
  await refreshList();
}

async function rename() {
  const to = ui.name.value.trim();
  if (!cur.name || !cur.rev) {
    note('Save the board first; then it can be renamed.', 'warn');
    return;
  }
  if (to === cur.name) {
    note('Type the new name in the name box, then press Rename.', 'warn');
    ui.name.focus();
    return;
  }
  if (!validName(to)) {
    note(ERRORS.invalid_name, 'warn');
    return;
  }
  const r = await bridge.call('/api/boards/rename', { method: 'POST', body: { from: cur.name, to } });
  if (!r.ok || !r.data || !r.data.board) {
    note(errText(r, 'The board could not be renamed.'), 'err');
    return;
  }
  const from = cur.name;
  cur.name = r.data.board.name;
  cur.rev = r.data.board.rev;
  if (kept.has(from)) { kept.set(cur.name, kept.get(from)); kept.delete(from); }
  api.updateScene({ appState: { name: cur.name }, captureUpdate: X.CaptureUpdateAction.NEVER });
  note(`Renamed to ${cur.name}.`, 'ok');
  renderTitle();
  await refreshList();
}

// Delete asks twice: the first press arms the button for a few seconds.
let armed = null;
async function remove() {
  if (!cur.name) return;
  if (armed !== cur.name) {
    armed = cur.name;
    ui.del.textContent = `Delete ${cur.name}?`;
    ui.del.classList.add('armed');
    setTimeout(() => { armed = null; ui.del.textContent = 'Delete'; ui.del.classList.remove('armed'); }, 4000);
    return;
  }
  armed = null;
  ui.del.textContent = 'Delete';
  ui.del.classList.remove('armed');
  const name = cur.name;
  const r = await bridge.call('/api/boards/delete', { method: 'POST', body: { name } });
  if (!r.ok) {
    note(errText(r, 'The board could not be deleted.'), 'err');
    return;
  }
  kept.delete(name);
  // The drawing stays on screen as an unsaved board, so a mistaken delete can still be saved again.
  cur.name = UNTITLED;
  cur.rev = null;
  cur.saved = -1;
  ui.name.value = name;
  note(`Deleted ${name}. What you see is still here until you open or start another board; Save keeps it again.`, 'ok');
  renderTitle();
  await refreshList();
}

// ---------- results onto the board ----------

const finished = () => bridge.jobs()
  .filter((j) => j.status === 'done' && (j.outputs || []).some((o) => o && /^\/outputs\//.test(o.url)))
  .sort((a, b) => (b.created_at || 0) - (a.created_at || 0))
  .slice(0, 24);

// What a result puts on the board: its picture, or for video, 3D and audio a still that links to the file.
function itemOf(job) {
  const outs = (job.outputs || []).filter((o) => o && /^\/outputs\//.test(o.url));
  const first = outs[0];
  const picture = outs.find((o) => o.kind === 'image');
  return {
    job: job.id,
    kind: first.kind,
    url: first.url,
    picture: picture ? picture.url : null,
    prompt: (job.prompt || '').slice(0, 300),
    model: bridge.jobName(job),
  };
}

let resultsSig = null;
function renderResults() {
  const jobs = finished();
  // Redrawn only when the finished results change, so their video previews do not reload on every poll.
  const sig = jobs.map((j) => j.id).join('|');
  if (sig === resultsSig) return;
  resultsSig = sig;
  ui.results.textContent = '';
  ui.resultsEmpty.hidden = jobs.length > 0;
  for (const job of jobs) {
    const it = itemOf(job);
    const thumb = it.kind === 'image' || it.picture
      ? el('img', { src: it.kind === 'image' ? it.url : it.picture, alt: '', loading: 'lazy', draggable: 'false' })
      : it.kind === 'video'
      ? el('video', { src: it.url, muted: true, preload: 'metadata', playsinline: true, 'aria-hidden': 'true' })
      : el('span', { class: 'board-res-kind' }, [it.kind === 'model' ? '3D' : 'Audio']);
    const kindLabel = { image: 'Picture', video: 'Video', model: '3D model', audio: 'Audio' }[it.kind] || it.kind;
    const place = el('button', { type: 'button', class: 'act' }, ['Place']);
    place.addEventListener('click', () => placeResult(it, null));
    const card = el('div', {
      class: 'board-res', draggable: 'true', role: 'group',
      'aria-label': `${kindLabel}: ${it.prompt || it.model}. Drag onto the board, or press Place.`,
    }, [
      el('div', { class: 'board-res-media' }, [thumb, el('span', { class: 'board-res-tag' }, [kindLabel])]),
      el('p', { class: 'board-res-prompt', title: it.prompt }, [it.prompt || it.model]),
      place,
    ]);
    card.addEventListener('dragstart', (e) => {
      e.dataTransfer.setData(RESULT_MIME, JSON.stringify(it));
      e.dataTransfer.effectAllowed = 'copy';
      card.classList.add('dragging');
    });
    card.addEventListener('dragend', () => card.classList.remove('dragging'));
    ui.results.append(card);
  }
}

function loadImage(src) {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error('picture'));
    img.src = src;
  });
}

function videoFrame(src) {
  return new Promise((resolve, reject) => {
    const v = document.createElement('video');
    v.muted = true;
    v.preload = 'auto';
    v.playsInline = true;
    const done = () => { v.removeAttribute('src'); v.load(); };
    v.onerror = () => { done(); reject(new Error('video')); };
    v.onloadedmetadata = () => { v.currentTime = Math.min(0.5, (v.duration || 1) / 3); };
    v.onseeked = () => {
      const c = document.createElement('canvas');
      c.width = v.videoWidth;
      c.height = v.videoHeight;
      c.getContext('2d').drawImage(v, 0, 0);
      done();
      resolve(c);
    };
    v.src = src;
  });
}

// A plain card for a result with no picture (3D without a thumbnail, audio).
function plainCard(label, text) {
  const c = document.createElement('canvas');
  c.width = 800;
  c.height = 450;
  const g = c.getContext('2d');
  g.fillStyle = '#1b1b1f';
  g.fillRect(0, 0, c.width, c.height);
  g.fillStyle = '#f2efe9';
  g.font = '600 56px system-ui, sans-serif';
  g.fillText(label, 48, 120);
  g.fillStyle = '#b9b5ad';
  g.font = '28px system-ui, sans-serif';
  const words = text.split(/\s+/);
  let line = '';
  let y = 190;
  for (const w of words) {
    if (g.measureText(line + w).width > 700) {
      g.fillText(line, 48, y);
      line = '';
      y += 40;
      if (y > 400) break;
    }
    line += w + ' ';
  }
  if (y <= 400) g.fillText(line, 48, y);
  return c;
}

// Shrinks a picture to MAX_SIDE and encodes it small enough to embed. Stills stay clean (a still can start a
// video); what they are is said by a label beside them.
function encode(src) {
  for (const side of [MAX_SIDE, 1200, 800, 512]) {
    const scale = Math.min(1, side / Math.max(src.width, src.height));
    const c = document.createElement('canvas');
    c.width = Math.max(1, Math.round(src.width * scale));
    c.height = Math.max(1, Math.round(src.height * scale));
    const g = c.getContext('2d');
    g.drawImage(src, 0, 0, c.width, c.height);
    let data = c.toDataURL('image/webp', 0.86);
    let mimeType = 'image/webp';
    if (!data.startsWith('data:image/webp')) {
      data = c.toDataURL('image/jpeg', 0.86);
      mimeType = 'image/jpeg';
    }
    if (data.length <= MAX_PICTURE_CHARS) return { dataURL: data, mimeType, width: c.width, height: c.height };
  }
  return null;
}

const KIND_NAME = { video: 'Video', model: '3D model', audio: 'Audio' };

// The picture for a result, and whether it is a real frame of it (a plain card never starts a video).
async function pictureFor(it) {
  if (it.kind === 'image') return { pic: encode(await loadImage(it.url)), real: true };
  // A still of a video or 3D model; a browser that cannot read the video (a codec it lacks) gets a plain card.
  try {
    if (it.kind === 'video') return { pic: encode(await videoFrame(it.url)), real: true };
    if (it.kind === 'model' && it.picture) return { pic: encode(await loadImage(it.picture)), real: true };
  } catch (_) { /* falls through to the card */ }
  return { pic: encode(plainCard(KIND_NAME[it.kind] || 'Result', it.prompt || it.model)), real: false };
}

const randomId = () => {
  const b = new Uint8Array(10);
  crypto.getRandomValues(b);
  return [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
};

async function placeResult(it, at) {
  if (!api) return;
  note('Placing…');
  let pic = null;
  let real = false;
  try {
    ({ pic, real } = await pictureFor(it));
  } catch (_) {
    pic = null;
  }
  if (!pic) {
    note('That result could not be turned into a picture for the board.', 'err');
    return;
  }
  const fileId = `agave-${randomId()}`;
  const st = api.getAppState();
  const w = Math.min(480, pic.width);
  const h = Math.round(w * (pic.height / pic.width));
  let x;
  let y;
  if (at) {
    const p = X.viewportCoordsToSceneCoords(at, st);
    x = p.x - w / 2;
    y = p.y - h / 2;
  } else {
    const p = X.viewportCoordsToSceneCoords({ clientX: st.offsetLeft + st.width / 2, clientY: st.offsetTop + st.height / 2 }, st);
    x = p.x - w / 2;
    y = p.y - h / 2;
  }
  api.addFiles([{ id: fileId, dataURL: pic.dataURL, mimeType: pic.mimeType, created: Date.now() }]);
  // The picture links back to the result's file; customData keeps where it came from (and, for a picture, the
  // original file, which is what Send to Video uses).
  const [image] = X.convertToExcalidrawElements([{ type: 'image', fileId, x, y, width: w, height: h }]);
  image.status = 'saved';
  image.link = it.url;
  image.customData = { agave: { job: it.job, kind: it.kind, url: it.url, picture: it.kind === 'image' ? it.url : null, still: real } };
  // A still of a video, 3D model or audio gets a label under it, grouped with it (shots skip labels).
  const parts = [image];
  if (it.kind !== 'image') {
    const words = (it.prompt || it.model || '').replace(/\s+/g, ' ').slice(0, 48);
    const [label] = X.convertToExcalidrawElements([{
      type: 'text', x, y: y + h + 8, text: `${KIND_NAME[it.kind] || 'Result'} · ${words}`, fontSize: 16,
    }]);
    label.customData = { agave: { label: true, job: it.job } };
    const group = `agave-${randomId()}`;
    image.groupIds = [group];
    label.groupIds = [group];
    parts.push(label);
  }
  // Dropped on a frame, the picture joins that frame's shot (Excalidraw keeps a frame's children just below it).
  const all = api.getSceneElementsIncludingDeleted();
  const cx = x + w / 2;
  const cy = y + h / 2;
  const frame = [...all].reverse().find((e) => !e.isDeleted && (e.type === 'frame' || e.type === 'magicframe')
    && cx >= e.x && cx <= e.x + e.width && cy >= e.y && cy <= e.y + e.height);
  let elements = [...all, ...parts];
  if (frame) {
    for (const e of parts) e.frameId = frame.id;
    const at2 = all.indexOf(frame);
    elements = [...all.slice(0, at2), ...parts, ...all.slice(at2)];
  }
  api.updateScene({ elements, captureUpdate: X.CaptureUpdateAction.IMMEDIATELY });
  api.setActiveTool({ type: 'selection' });
  const where = frame ? ` in ${(frame.name || '').trim() || 'the frame'}` : '';
  note(it.kind === 'image' ? `Placed the picture${where}.` : `Placed a still of the ${KIND_NAME[it.kind] ? KIND_NAME[it.kind].toLowerCase() : 'result'}${where}; its link opens the file.`, 'ok');
}

function onDrop(e) {
  if (!e.dataTransfer) return;
  if (![...e.dataTransfer.types].includes(RESULT_MIME)) {
    onFileDrop(e);
    return;
  }
  e.preventDefault();
  e.stopPropagation();
  ui.canvas.classList.remove('drop');
  let it = null;
  try { it = JSON.parse(e.dataTransfer.getData(RESULT_MIME)); } catch (_) { it = null; }
  // Only results the studio itself lists can be dropped.
  const job = it && bridge.jobs().find((j) => j.id === it.job && j.status === 'done');
  if (!job) return;
  placeResult(itemOf(job), { clientX: e.clientX, clientY: e.clientY });
}

function onDragOver(e) {
  if (!e.dataTransfer || ![...e.dataTransfer.types].includes(RESULT_MIME)) return;
  e.preventDefault();
  e.stopPropagation();
  e.dataTransfer.dropEffect = 'copy';
  ui.canvas.classList.add('drop');
}

// ---------- frames become shots ----------

const readingOrder = (a, b) => (Math.abs(a.y - b.y) > Math.min(a.height, b.height) / 2 ? a.y - b.y : a.x - b.x);

function shots() {
  if (!api) return [];
  const all = api.getSceneElements();
  const frames = all.filter((e) => e.type === 'frame' || e.type === 'magicframe').sort(readingOrder);
  const files = api.getFiles();
  return frames.map((f, i) => {
    const inside = all.filter((e) => e.frameId === f.id);
    const prompt = inside.filter((e) => e.type === 'text' && !(e.customData && e.customData.agave && e.customData.agave.label)).sort(readingOrder)
      .map((e) => (e.originalText || e.text || '').trim()).filter(Boolean).join('\n');
    const img = inside.filter((e) => e.type === 'image' && e.fileId && files[e.fileId]
      && !(e.customData && e.customData.agave && e.customData.agave.still === false)).sort(readingOrder)[0] || null;
    return { id: f.id, name: (f.name || '').trim() || `Shot ${i + 1}`, prompt, image: img, file: img ? files[img.fileId] : null };
  });
}

function renderShots() {
  if (!ui) return;
  const list = shots();
  ui.shots.textContent = '';
  ui.shotsEmpty.hidden = list.length > 0;
  list.forEach((s) => {
    const send = el('button', { type: 'button', class: 'small-btn' }, ['Send to Video']);
    send.addEventListener('click', () => sendToVideo(s, send));
    const prompt = el('p', { class: 'board-shot-prompt' }, [s.prompt || 'No words in this frame yet.']);
    if (!s.prompt) prompt.classList.add('muted');
    ui.shots.append(el('li', { class: 'board-shot' }, [
      safePicture(s.file) ? el('img', { src: s.file.dataURL, alt: '' }) : el('span', { class: 'board-shot-none' }, ['No picture']),
      el('div', { class: 'board-shot-body' }, [el('b', {}, [s.name]), prompt, send]),
    ]));
  });
}

// The picture that starts the video: the result's own file when it came from the studio, else the board's copy,
// saved on the engine like any upload (PNG, JPEG or WebP).
async function startPicture(s) {
  const own = s.image.customData && s.image.customData.agave && s.image.customData.agave.picture;
  if (typeof own === 'string' && /^\/outputs\/[A-Za-z0-9._-]+$/.test(own)) return own;
  if (!safePicture(s.file)) throw new Error('picture');
  let data = s.file.dataURL;
  if (!/^data:image\/(png|jpeg|webp);base64,/.test(data)) {
    const enc = encode(await loadImage(data));
    if (!enc) throw new Error('picture');
    data = enc.dataURL;
  }
  const r = await bridge.call('/api/uploads', { method: 'POST', body: { data } });
  if (r.status !== 201 || !r.data || !r.data.url) throw new Error('upload');
  return r.data.url;
}

async function sendToVideo(s, btn) {
  if (!s.prompt && !s.image) {
    note(`${s.name} has no words or picture yet. Write the shot inside the frame first.`, 'warn');
    return;
  }
  btn.disabled = true;
  let image = null;
  try {
    if (s.image) image = await startPicture(s);
  } catch (_) {
    btn.disabled = false;
    note(`The picture in ${s.name} could not be used as a start picture.`, 'err');
    return;
  }
  btn.disabled = false;
  // Fills the Video form only. Nothing runs until Generate is pressed there.
  bridge.sendToVideo({ name: s.name, prompt: s.prompt, image });
}

// ---------- the view ----------

function buildUi(host) {
  const name = el('input', { id: 'boardName', class: 'board-name', maxlength: '60', placeholder: 'Board name', spellcheck: 'false', 'aria-label': 'Board name', autocomplete: 'off' });
  const open = el('select', { id: 'boardOpen', class: 'plain-select board-open', 'aria-label': 'Open a board' });
  const btn = (label, title) => el('button', { type: 'button', class: 'small-btn', title }, [label]);
  const nw = btn('New', 'Start an empty board (unsaved changes on this one are kept)');
  const sv = btn('Save', 'Save this board under the name in the box');
  const rn = btn('Rename', 'Give the saved board the name in the box');
  const dl = btn('Delete', 'Delete the saved board');
  dl.classList.add('board-del');
  const stateText = el('span', { class: 'board-state', 'aria-live': 'polite' });
  const noteEl = el('p', { class: 'board-note', role: 'status' });
  const canvas = el('div', { class: 'board-canvas', 'aria-label': 'Board' });
  const loadingNote = el('p', { class: 'board-loading muted' }, ['Loading the board editor…']);
  canvas.append(loadingNote);
  const results = el('div', { class: 'board-results', role: 'list', 'aria-label': 'Finished results' });
  const resultsEmpty = el('p', { class: 'muted small' }, ['Finished pictures, videos and 3D models show here. Drag one onto the board, or press Place.']);
  const shotsList = el('ol', { class: 'board-shots' });
  const shotsEmpty = el('p', { class: 'muted small' }, ['Draw a frame (the frame tool, F) around a sketch or a picture and write the shot inside it. Each frame lists here as a shot.']);
  host.append(
    el('div', { class: 'board-bar' }, [open, name, nw, sv, rn, dl, stateText]),
    noteEl,
    el('div', { class: 'board-main' }, [
      canvas,
      el('aside', { class: 'board-side' }, [
        el('section', { class: 'panel-box', 'aria-labelledby': 'boardShotsTitle' }, [
          el('h3', { id: 'boardShotsTitle' }, ['Shots']),
          el('p', { class: 'muted small' }, ['Send to Video fills the Video form with the frame\'s words and first picture. Nothing runs until you press Generate.']),
          shotsEmpty, shotsList,
        ]),
        el('section', { class: 'panel-box', 'aria-labelledby': 'boardResTitle' }, [
          el('h3', { id: 'boardResTitle' }, ['Results']),
          resultsEmpty, results,
        ]),
      ]),
    ]),
  );
  ui = { name, open, save: sv, rename: rn, del: dl, state: stateText, note: noteEl, canvas, loadingNote, results, resultsEmpty, shots: shotsList, shotsEmpty };
  open.addEventListener('change', () => {
    const v = open.value;
    if (v === 'new:') openBoard(UNTITLED);
    else if (v.startsWith('b:')) openBoard(v.slice(2));
  });
  nw.addEventListener('click', newBoard);
  sv.addEventListener('click', () => save(false));
  rn.addEventListener('click', rename);
  dl.addEventListener('click', remove);
  name.addEventListener('keydown', (e) => { if (e.key === 'Enter') { e.preventDefault(); save(false); } });
  canvas.addEventListener('dragover', onDragOver, true);
  canvas.addEventListener('dragleave', () => canvas.classList.remove('drop'));
  canvas.addEventListener('drop', onDrop, true);
  window.addEventListener('paste', onPaste, true);
  window.addEventListener('beforeunload', (e) => {
    if (hasUnsaved()) { e.preventDefault(); e.returnValue = ''; }
  });
}

/** Shows the board view in `host`. `b` = { call, jobs, jobName, sendToVideo }. */
export async function show(host, b) {
  bridge = b;
  if (!ui) buildUi(host);
  renderResults();
  refreshList();
  if (!api) {
    try {
      await loadEditor();
    } catch (_) {
      ui.loadingNote.textContent = 'The board editor could not load. Reload the page; if that does not help, check that web/vendor/excalidraw is complete.';
      return;
    }
    if (!api && !ui.canvas.dataset.mounted) {
      ui.canvas.dataset.mounted = '1';
      ui.loadingNote.remove();
      mountEditor();
      renderTitle();
      renderList();
    }
  } else {
    // A hidden editor lost its place on the page: let it measure again.
    requestAnimationFrame(() => api.refresh());
  }
}

/** The studio's jobs changed (a new result finished). */
export function jobsChanged() {
  if (ui) renderResults();
}
