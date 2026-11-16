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
    const apiPollRequests = [];
    const wsFrames = [];

    page.on('pageerror', (error) => errors.push(String(error)));
    page.on('request', (req) => {
      if (req.url().includes('/api/state') || req.url().includes('/api/planets')) {
        apiPollRequests.push(req.url());
      }
    });
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
    await page.getByRole('heading', { name: '地球', exact: true }).waitFor();
    await page.getByText('Rust 内核 · WebSocket 推送', { exact: true }).waitFor();
    await page.getByTestId('surface-canvas').waitFor();

    // 1. Verify initial state: Polity central treasury (2,600), 12 land sectors + 4 oceans,
    //    local market price, CBD hosting (1/6), and 7-profession Pop breakdown!
    const surfaceFrames = wsFrames.filter((f) => f.kind === 'planet_surface');
    assert.equal(surfaceFrames.length, 1);
    const surf = surfaceFrames[0].surface;
    assert.equal(surf.regions.filter((r) => !r.is_ocean).length, 12);
    assert.equal(surf.regions.filter((r) => r.is_ocean).length, 4);

    const initialTreasury = Number(
      (await page.getByTestId('polity-treasury').textContent()).replace(/,/g, ''),
    );
    assert.equal(initialTreasury, 2600);
    assert.equal(await page.getByTestId('planet-jobs').textContent(), '540/540');
    assert.ok((await page.getByTestId('sector-cbd-hosting').textContent()).includes('1/6'));

    await page.getByTestId('toggle-pops-detail').click();
    const popsTableText = await page.getByTestId('sector-pops-table').textContent();
    assert.equal(popsTableText.includes('家属'), false, 'dependents field must be removed');
    for (const role of [
      '个体民',
      '流民',
      '劳工 · 待业',
      '劳工 · 国有',
      '劳工 · 民间私产',
      '工程师 · 国有',
      '学者 · 国有',
      '官僚 · 国有',
      '官僚 · 民间私产',
      '军人 · 国有',
      '资本家 · 民间私产',
    ]) {
      assert.ok(popsTableText.includes(role), `missing employer-qualified pop role ${role} in ${popsTableText}`);
    }
    const wageStripText = await page.getByTestId('sector-wage-strip').textContent();
    assert.ok(wageStripText.includes('学者 130.00') && wageStripText.includes('劳工 60.00'), `unexpected wage strip: ${wageStripText}`);

    if (shotDir) {
      await page.screenshot({ path: path.join(shotDir, 'surface-regions-1280.png') });
    }

    // 2. Pick an empty land tile: verify Secondary slots are locked until Primary is built (`1主导+3次要`),
    //    then build a Primary `2x` dominant orchard (-300 treasury)!
    await page.getByTestId('pick-empty-tile').click();
    await page.getByTestId('secondary-locked-hint').waitFor();
    await page.getByTestId('build-primary-orchard').click();
    await page.waitForFunction(
      () => document.querySelector('[data-testid="slot-primary"]')?.textContent?.includes('2x 主导规模'),
      null,
      { timeout: 3000 },
    );

    // 3. Jump back to Capital tile, build 1 State Orchard on Secondary2 (-150), privatize Secondary0 (+120),
    //    advance 2 months at 4x so private pool accumulates dividends, privatize Secondary2, then found a Corporate Campus!
    await page.getByTestId('jump-capital-tile').click();
    await page.getByTestId('build-secondary2-orchard').click();
    await page.getByTestId('privatize-secondary0').waitFor();
    await page.getByTestId('privatize-secondary0').click();
    await page.waitForFunction(
      () => document.querySelector('[data-testid="sector-cbd-hosting"]')?.textContent?.includes('2/6'),
      null,
      { timeout: 3000 },
    );

    const initialPlanetPop = Number(
      (await page.getByTestId('planet-population').textContent()).replace(/,/g, ''),
    );
    await page.getByRole('button', { name: '4x', exact: true }).click();
    await page.waitForFunction(
      (prevPop) => {
        const rawPop = document.querySelector('[data-testid="planet-population"]')?.textContent;
        const rawPool = document.querySelector('[data-testid="sector-investment-pool"]')?.textContent;
        if (!rawPop || !rawPool) return false;
        return Number(rawPop.replace(/,/g, '')) > prevPop && Number(rawPool.replace(/,/g, '')) >= 120;
      },
      initialPlanetPop,
      { timeout: 5000 },
    );
    await page.keyboard.press('Space');
    await page.getByText('已暂停', { exact: true }).waitFor();

    // Order book: after at least one monthly settlement the closing depth / trades are visible.
    const bookText = await page.getByTestId('sector-order-book').textContent();
    assert.ok(/长协 \d+ · 现货 \d+/.test(bookText), `order book summary missing: ${bookText}`);
    const priceText = await page.getByTestId('sector-apple-price').textContent();
    assert.ok(/^\d+\.\d{2}$/.test(priceText.trim()), `price must be shown in credits with cents: ${priceText}`);
    const settledFrame = wsFrames.filter((f) => f.kind === 'planet_view' && f.planet).pop();
    const tradedVolume = settledFrame.planet.sectors.reduce(
      (acc, s) => acc + s.order_book.last_contract_volume + s.order_book.last_spot_volume,
      0,
    );
    assert.ok(tradedVolume > 0, 'apples must change hands on the order books after a settlement');

    // Depot and central-bank policy toggles.
    const depotBefore = await page.getByTestId('cycle-depot-policy').textContent();
    await page.getByTestId('cycle-depot-policy').click();
    await page.waitForFunction(
      (prev) => document.querySelector('[data-testid="cycle-depot-policy"]')?.textContent !== prev,
      depotBefore,
      { timeout: 3000 },
    );
    const mpBefore = await page.getByTestId('cycle-monetary-policy').textContent();
    await page.getByTestId('cycle-monetary-policy').click();
    await page.waitForFunction(
      (prev) => document.querySelector('[data-testid="cycle-monetary-policy"]')?.textContent !== prev,
      mpBefore,
      { timeout: 3000 },
    );

    // Privatize Secondary2 (or if autonomous private build already added a 3rd private orchard, check `can_found_enterprise`)!
    const cbdTextAfter = await page.getByTestId('sector-cbd-hosting').textContent();
    if (cbdTextAfter.includes('2/6')) {
      await page.getByTestId('privatize-secondary2').click();
    }
    await page.waitForFunction(
      () => {
        const btn = document.querySelector('[data-testid="region-build-campus"]');
        return btn && !btn.hasAttribute('disabled');
      },
      null,
      { timeout: 3000 },
    );

    // Found Corporate Campus (`企业园区`) via macro button and verify named Enterprise appears & CBD hosting slots free up!
    await page.getByTestId('region-build-campus').click();
    await page.getByTestId('sector-enterprises').waitFor();
    const entText = await page.getByTestId('sector-enterprises').textContent();
    assert.ok(entText.includes('农垦集团'), `expected named enterprise, got ${entText}`);

    // Static Voronoi surface must NEVER be resent on build/economy updates!
    assert.equal(wsFrames.filter((f) => f.kind === 'planet_surface').length, 1);
    assert.deepEqual(apiPollRequests, []);

    await page.getByRole('button', { name: '自然地形', exact: true }).click();
    if (shotDir) {
      await page.screenshot({ path: path.join(shotDir, 'surface-terrain-1280.png') });
    }
    await page.getByRole('button', { name: '大区染色', exact: true }).click();

    // 4. Compact viewport check (640x600) — zero horizontal overflow!
    await page.setViewportSize({ width: 640, height: 600 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false);
    if (shotDir) {
      await page.screenshot({ path: path.join(shotDir, 'surface-compact-640.png') });
    }
    assert.deepEqual(errors, []);
    console.log('PASS: order book depth + depot/central-bank policy toggles.');
    console.log('PASS: Polity treasury + Sector local markets + 7-role Pops + Primary-first 2x slots + Privatization & Corporate Campus.');

    // 5. WebSocket disconnect & reconnect recovery.
    let blockWs = true;
    await page.routeWebSocket('**/api/ws', (ws) => {
      if (blockWs) {
        ws.close({ code: 1013, reason: 'Unavailable' });
      } else {
        ws.connectToServer();
      }
    });
    await page.reload();
    await page.getByRole('heading', { name: '无法读取星球' }).waitFor();
    blockWs = false;
    await page.getByRole('button', { name: '重新连接' }).click();
    await page.getByRole('heading', { name: '地球', exact: true }).waitFor();
    console.log('PASS: WebSocket disconnect error panel and reconnect recovery.');
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
