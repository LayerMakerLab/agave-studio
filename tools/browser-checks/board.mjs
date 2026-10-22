// The Board view end to end, at desktop and phone width: draw a frame with words, drag a picture result into it,
// place a video, save, switch away and back, send the shot to Video (which must start no job), rename, delete.
import { BASE, launch, studioPage, api, openBoardView, deleteAllBoards, check, finish } from './lib.mjs';

const OUT = process.env.SCREENSHOTS || '';
const browser = await launch();
const page = await studioPage(browser, 'desktop');
await page.goto(`${BASE}/app/`);

// Results from the fake providers (nothing is paid in test mode): two pictures and a video.
const seeded = await page.evaluate(async () => {
  const post = (body) => fetch('/api/generate', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) }).then((r) => r.json());
  const jobs = await Promise.all([
    post({ model: 'fal/flux-schnell', mode: 't2i', prompt: 'a red fox in snow, wide shot' }),
    post({ model: 'fal/flux-schnell', mode: 't2i', prompt: 'a lighthouse at dusk' }),
    post({ model: 'fal/kling-3-pro-t2v', mode: 't2v', prompt: 'waves crash on rocks', duration: 5 }),
  ]);
  return jobs.map((j) => j.job && j.job.id);
});
await page.waitForFunction(async (ids) => {
  const r = await fetch('/api/jobs').then((x) => x.json());
  return ids.every((id) => r.jobs.some((j) => j.id === id && j.status === 'done'));
}, seeded, { timeout: 60000, polling: 1000 });

await openBoardView(page);
await deleteAllBoards(page);
await page.waitForSelector('.board-res', { timeout: 15000 });
const jobsBefore = (await api(page, '/api/jobs')).data.jobs.length;
const note = () => page.textContent('.board-note');
const state = () => page.textContent('.board-state');

// Draw a frame (the first key after load goes to focusing the editor), write the shot in it.
const box = await page.locator('.board-canvas').boundingBox();
await page.locator('[data-testid="toolbar-selection"]').click({ force: true });
await page.mouse.click(box.x + 900, box.y + 600);
await page.keyboard.press('Escape');
await page.waitForTimeout(300);
await page.keyboard.press('f');
await page.waitForTimeout(200);
await page.mouse.move(box.x + 380, box.y + 110);
await page.mouse.down();
for (let i = 1; i <= 12; i++) await page.mouse.move(box.x + 380 + i * 45, box.y + 110 + i * 35);
await page.mouse.up();
await page.keyboard.press('Escape');
await page.mouse.dblclick(box.x + 420, box.y + 150);
await page.waitForSelector('textarea.excalidraw-wysiwyg');
await page.keyboard.type('Slow push in on the fox as snow falls');
await page.keyboard.press('Escape');
await page.waitForTimeout(400);

// Drag the fox picture into the frame; Place the video.
await page.locator('.board-res').filter({ hasText: 'fox' }).first().dragTo(page.locator('.board-canvas'), { targetPosition: { x: 650, y: 360 } });
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Placed the picture'), null, { timeout: 15000 });
check(/in the frame/.test(await note()), 'a dragged picture lands in the frame');
await page.locator('.board-res').filter({ hasText: 'waves' }).first().getByRole('button', { name: 'Place' }).click();
await page.waitForFunction(() => /still of the video/.test(document.querySelector('.board-note').textContent), null, { timeout: 20000 });
await page.waitForTimeout(600);
const shots = await page.evaluate(() => [...document.querySelectorAll('.board-shot')].map((li) => ({
  prompt: li.querySelector('.board-shot-prompt').textContent, picture: !!li.querySelector('img'),
})));
check(shots.length === 1 && shots[0].prompt === 'Slow push in on the fox as snow falls' && shots[0].picture, 'the frame lists as a shot with its words and picture');
if (OUT) await page.screenshot({ path: `${OUT}/board-desktop.png` });

// Hidden: the library's Browse libraries link and the web-embed tool.
await page.locator('.App-toolbar__extra-tools-trigger').click({ force: true });
await page.waitForTimeout(300);
check(await page.isVisible('[data-testid="toolbar-frame"]') && (await page.locator('[data-testid="toolbar-embeddable"]').count()) > 0
  && !(await page.isVisible('[data-testid="toolbar-embeddable"]')), 'the extra tools open with the web-embed tool hidden');
await page.keyboard.press('Escape');
await page.locator('.sidebar-trigger').first().click({ force: true });
await page.waitForTimeout(500);
check(await page.isVisible('.layer-ui__library') && (await page.locator('.library-menu-browse-button').count()) > 0 && !(await page.isVisible('.library-menu-browse-button')), 'the library opens with Browse libraries hidden');
await page.locator('.sidebar-trigger').first().click({ force: true });
await page.waitForTimeout(300);

// Save, and the engine has it.
await page.fill('#boardName', 'Fox storyboard');
await page.click('.board-bar button:text-is("Save")');
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Saved'), null, { timeout: 15000 });
const saved = (await api(page, '/api/boards/load?name=Fox%20storyboard')).data.board;
const kinds = saved.elements.filter((e) => !e.isDeleted).map((e) => e.type).sort().join(',');
check(kinds === 'frame,image,image,text,text', `the saved board holds the frame, two pictures, the words and a label (${kinds})`);
check(saved.elements.filter((e) => e.type === 'image').every((e) => /^\/outputs\//.test(e.link)), 'each picture links to its result file');

// Unsaved changes survive switching away to the studio and back, and opening another board.
await page.locator('[data-testid="toolbar-rectangle"]').click({ force: true });
await page.mouse.move(box.x + 760, box.y + 560);
await page.mouse.down();
await page.mouse.move(box.x + 860, box.y + 640, { steps: 4 });
await page.mouse.up();
await page.keyboard.press('Escape');
check((await state()).includes('unsaved changes'), 'a new change shows as unsaved');
await page.click('.mode[data-mode="image"]');
await page.waitForTimeout(300);
check(await page.isVisible('#gallery'), 'Image shows the gallery again');
await page.click('#boardBtn');
await page.waitForTimeout(500);
check((await state()).includes('unsaved changes'), 'coming back keeps the change');
await page.click('.board-bar button:text-is("New")');
await page.waitForTimeout(300);
await page.selectOption('#boardOpen', 'b:Fox storyboard');
await page.waitForTimeout(800);
check(/changes you had not saved/.test(await note()), 'reopening brings the unsaved change back');

// Send to Video fills the Video form and starts no job.
await page.locator('.board-shot').first().getByRole('button', { name: 'Send to Video' }).click();
await page.waitForFunction(() => document.querySelector('.mode[data-mode="video"]').getAttribute('aria-pressed') === 'true', null, { timeout: 15000 });
const form = await page.evaluate(() => ({ prompt: document.querySelector('#prompt').value, tray: [...document.querySelectorAll('#tray img')].map((i) => i.getAttribute('src')) }));
check(form.prompt === 'Slow push in on the fox as snow falls' && form.tray.length === 1 && /^\/outputs\/[0-9a-f]+\.png$/.test(form.tray[0]), 'the Video form has the words and the picture\'s own file');
await page.waitForTimeout(1500);
check((await api(page, '/api/jobs')).data.jobs.length === jobsBefore, 'no job was started');
if (OUT) await page.screenshot({ path: `${OUT}/video-form-desktop.png` });

// Rename and delete.
await page.click('#boardBtn');
await page.waitForTimeout(400);
await page.fill('#boardName', 'Fox boards (v2)');
await page.click('.board-bar button:text-is("Rename")');
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Renamed'), null, { timeout: 10000 });
check(JSON.stringify((await api(page, '/api/boards')).data.boards.map((b) => b.name)) === '["Fox boards (v2)"]', 'rename renames it on the engine');
await page.click('.board-bar .board-del');
await page.click('.board-bar .board-del');
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Deleted'), null, { timeout: 10000 });
check((await api(page, '/api/boards')).data.boards.length === 0, 'delete deletes it');
await page.fill('#boardName', 'Fox storyboard');
await page.click('.board-bar button:text-is("Save")');
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Saved'), null, { timeout: 15000 });

// Phone width.
const phoneBrowser = browser;
const pp = await studioPage(phoneBrowser, 'phone', { width: 390, height: 844 });
await openBoardView(pp);
await pp.selectOption('#boardOpen', 'b:Fox storyboard');
await pp.waitForTimeout(1200);
const scroll = await pp.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth]);
check(scroll[0] === scroll[1], `no sideways scroll at 390 px (${scroll[0]} / ${scroll[1]})`);
check((await pp.textContent('.board-state')).startsWith('Fox storyboard'), 'the saved board opens on a phone');
if (OUT) await pp.screenshot({ path: `${OUT}/board-phone.png` });

await deleteAllBoards(page);
await browser.close();
finish();
