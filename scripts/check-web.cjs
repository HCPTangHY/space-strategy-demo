// Run with Playwright installed outside the source tree:
// NODE_PATH=/path/to/node_modules node scripts/check-web.cjs [URL] [SCREENSHOT_DIR]
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

(async () => {
  const url = process.argv[2] || 'https://files.zhenxia.top/space-strategy/';
  const shotDir = process.argv[3] || '';
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox'] });
  try {
    const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
    const errors = [];
    const wsFrames = [];

    page.on('pageerror', (error) => errors.push(String(error)));
    page.on('websocket', (ws) => {
      ws.on('framereceived', (frame) => {
        if (typeof frame.payload === 'string') {
          try {
            wsFrames.push(JSON.parse(frame.payload));
          } catch {}
        }
      });
    });

    await page.goto(url);
    await page.getByText('Rust 内核 · WebSocket 推送', { exact: true }).waitFor();

    // 1. No window pops up until clicking Earth!
    assert.equal(await page.getByTestId('planet-window').count(), 0);
    await page.getByTestId('earth-globe-trigger').click();
    await page.getByTestId('planet-window').waitFor();
    await page.getByTestId('surface-canvas').waitFor();
    assert.equal(await page.getByTestId('surface-canvas').getAttribute('data-map-level'), 'region');

    // 2. Level 1 click opens Region Window (NOT Tile Window)
    const canvasBox = await page.getByTestId('surface-canvas').boundingBox();
    await page.mouse.click(canvasBox.x + canvasBox.width * 0.5, canvasBox.y + canvasBox.height * 0.5);
    await page.getByTestId('region-window').waitFor();
    assert.equal(await page.getByTestId('tile-window').count(), 0);

    // 3. Zoom into Level 2 -> opens Tile Window on click!
    await page.getByTestId('zoom-into-region').click();
    await page.waitForTimeout(150);
    assert.equal(await page.getByTestId('surface-canvas').getAttribute('data-map-level'), 'tile');
    await page.mouse.click(canvasBox.x + canvasBox.width * 0.5, canvasBox.y + canvasBox.height * 0.5);
    await page.getByTestId('tile-window').waitFor();

    // 4. Jump to an empty land tile: general secondary slots are unlocked immediately,
    //    while prerequisite-bound `base:commercial_street` stays gated on `required_primary_tags`!
    await page.getByTestId('pick-empty-tile').click();
    await page.waitForTimeout(150);
    assert.equal(await page.getByTestId('secondary-locked-hint').count(), 0);
    await page.getByTestId('select-build-secondary0').waitFor();
    const secOptions = await page
      .getByTestId('select-build-secondary0')
      .locator('option')
      .evaluateAll((opts) => opts.map((o) => o.value));
    assert.ok(secOptions.length > 0, 'empty land tile must offer general 1x secondary buildings');
    assert.ok(
      !secOptions.includes('base:commercial_street'),
      'prerequisite-bound commercial street must NOT appear when primary lacks required tags',
    );
    await page.getByTestId('build-secondary0-submit').click();
    await page.getByTestId('demolish-secondary0').waitFor();

    // 5. Regression test: while time is running (`4x`), zoom back from Level 2 -> Level 1
    //    via mouse wheel AND verify live ticks NEVER snap the map back to Level 2!
    await page.getByRole('button', { name: '4x' }).click();
    await page.mouse.move(canvasBox.x + canvasBox.width * 0.5, canvasBox.y + canvasBox.height * 0.5);
    for (let i = 0; i < 8; i++) {
      await page.mouse.wheel(0, 480);
    }
    await page.waitForTimeout(150);
    assert.equal(await page.getByTestId('surface-canvas').getAttribute('data-map-level'), 'region');
    const framesBeforeWait = wsFrames.length;
    await page.waitForTimeout(1400);
    assert.ok(wsFrames.length > framesBeforeWait, 'live ticks must arrive while time is running');
    assert.equal(
      await page.getByTestId('surface-canvas').getAttribute('data-map-level'),
      'region',
      'zooming back to Level 1 while time is running must NEVER snap back to Level 2',
    );
    await page.getByRole('button', { name: '暂停' }).click();

    // 6. Scenario switch (`殖民地开局` -> frontier `base:admin` claiming -> switch back to `地球开局`)!
    await page.getByTestId('scenario-select').selectOption('colony');
    await page.getByText('已切换至「殖民地开局」并开启新对局。', { exact: true }).waitFor();
    await page.waitForTimeout(200);
    const adminInit = await page.getByTestId('polity-admin-quota').textContent();
    assert.equal(adminInit?.trim(), '9/22', 'Colony Start should begin at 9/22 direct-rule admin quota');

    await page.mouse.click(canvasBox.x + canvasBox.width * 0.5, canvasBox.y + canvasBox.height * 0.5);
    await page.getByTestId('region-window').waitFor();
    const allOpts = await page
      .locator('.region-select[aria-label="切换当前大区"] option')
      .evaluateAll((opts) => opts.map((o) => o.value));
    let claimedExpanded = false;
    for (const val of allOpts) {
      await page.locator('.region-select[aria-label="切换当前大区"]').selectOption(val);
      const btn = page.getByTestId('region-claim-frontier');
      if ((await btn.count()) > 0 && !(await btn.isDisabled())) {
        await btn.click();
        await page.waitForTimeout(200);
        const adminAfter = await page.getByTestId('polity-admin-quota').textContent();
        assert.notEqual(adminAfter?.trim(), '9/22', 'claiming frontier must increment admin_used');
        claimedExpanded = true;
        break;
      }
    }
    assert.ok(claimedExpanded, 'must find at least one region with claimable contiguous wilderness');

    await page.getByTestId('scenario-select').selectOption('earth');
    await page.getByText('已切换至「地球开局」并开启新对局。', { exact: true }).waitFor();

    if (shotDir) {
      await page.screenshot({ path: path.join(shotDir, 'desktop-1280.png') });
    }
    assert.deepEqual(errors, []);
    console.log('Playwright two-level verification OK, wsFrames=', wsFrames.length);
  } finally {
    await browser.close();
  }
})();
