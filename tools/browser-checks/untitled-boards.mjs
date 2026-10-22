// Unsaved untitled boards are never thrown away (fit review B1, finding 3): New after drawing on an untitled board,
// New again after another untitled one, and Delete of a saved board each keep every unsaved drawing in the Open list.
import { launch, studioPage, api, openBoardView, deleteAllBoards, check, finish } from './lib.mjs';

const browser = await launch();
const page = await studioPage(browser, 'untitled');
await openBoardView(page);
await deleteAllBoards(page);
await page.reload();
await page.waitForSelector('.excalidraw .excalidraw__canvas', { timeout: 30000 });
await page.waitForTimeout(500);

const box = await page.locator('canvas.interactive').boundingBox();
let shift = 0;
async function draw() {
  await page.locator('[data-testid="toolbar-rectangle"]').click({ force: true });
  const x = box.x + 400 + shift;
  shift += 40;
  await page.mouse.move(x, box.y + 200);
  await page.mouse.down();
  for (let i = 1; i <= 8; i++) await page.mouse.move(x + i * 15, box.y + 200 + i * 12);
  await page.mouse.up();
  await page.keyboard.press('Escape');
  await page.waitForTimeout(300);
}
const state = () => page.textContent('.board-state');
const options = () => page.$$eval('#boardOpen option', (o) => o.map((x) => x.textContent));
const valueOf = (label) => page.$$eval('#boardOpen option', (o, l) => (o.find((x) => x.textContent === l) || {}).value, label);

// 1. Draw on the first untitled board, then New: the drawing stays in the list.
await draw();
check((await state()).includes('unsaved changes'), 'the first untitled board has unsaved changes');
await page.getByRole('button', { name: 'New', exact: true }).click();
await page.waitForTimeout(300);
check((await options()).includes('Untitled board 1 (unsaved)'), 'New keeps the unsaved untitled board');
check(!(await state()).includes('unsaved'), 'and starts an empty one');

// 2. Draw on the second, then New again: both untitled drawings stay.
await draw();
const saved = await api(page, '/api/boards/save', { name: 'Saved one', board: { type: 'excalidraw', elements: [] }, create: true });
check(saved.status === 200, 'a saved board exists');
await page.getByRole('button', { name: 'New', exact: true }).click();
await page.waitForTimeout(300);
let opts = await options();
check(opts.includes('Untitled board 1 (unsaved)') && opts.includes('Untitled board 2 (unsaved)'), 'two untitled drawings are kept side by side');

// 3. Each one comes back with its changes.
await page.selectOption('#boardOpen', await valueOf('Untitled board 1 (unsaved)'));
await page.waitForTimeout(500);
check((await state()).startsWith('Untitled board 1') && (await state()).includes('unsaved changes'), 'Untitled board 1 comes back unsaved');
opts = await options();
check(opts.includes('Untitled board 2 (unsaved)') && opts.includes('Untitled board 3 (unsaved)') === false, 'the others stay listed');

// 4. Delete a saved board while untitled drawings are kept: none is lost. (Leaving the board and coming back
// lists the board saved through the API.)
await page.click('.mode[data-mode="image"]');
await page.click('#boardBtn');
await page.waitForTimeout(800);
await page.selectOption('#boardOpen', 'b:Saved one');
await page.waitForTimeout(800);
await draw();
await page.click('.board-bar .board-del');
await page.getByRole('button', { name: /Delete Saved one/ }).click();
await page.waitForFunction(() => document.querySelector('.board-note').textContent.startsWith('Deleted'), null, { timeout: 10000 });
await page.getByRole('button', { name: 'New', exact: true }).click();
await page.waitForTimeout(300);
opts = await options();
const untitled = opts.filter((o) => /^Untitled board \d+ \(unsaved\)$/.test(o));
check(untitled.length === 3, `the deleted board's drawing is kept beside the other untitled ones (${untitled.join(', ')})`);

// 5. Leaving the page still asks while anything is unsaved.
const asks = await page.evaluate(() => {
  const e = new Event('beforeunload', { cancelable: true });
  window.dispatchEvent(e);
  return e.defaultPrevented;
});
check(asks, 'leaving the page asks first');

await deleteAllBoards(page);
await browser.close();
finish();
