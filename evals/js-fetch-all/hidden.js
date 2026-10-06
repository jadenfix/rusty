const assert = require("assert");
const path = require("path");
const { fetchAll } = require(path.resolve("fetch_all.js"));
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  assert.deepStrictEqual(await fetchAll([], async () => 1), []);
  // Never more than `concurrency` in flight, and the pool refills promptly.
  let live = 0, peak = 0;
  const started = [];
  const t0 = Date.now();
  const ids = Array.from({ length: 12 }, (_, i) => i);
  const out = await fetchAll(ids, async (i) => {
    started.push(i);
    live++; peak = Math.max(peak, live);
    await sleep(i === 0 ? 120 : 20);
    live--;
    return `r${i}`;
  }, { concurrency: 3 });
  assert.deepStrictEqual(out, ids.map((i) => `r${i}`));
  assert.strictEqual(peak, 3, `peak ${peak}`);
  assert.ok(Date.now() - t0 < 200, "a slow call must not hold up the others (no fixed batches)");
  // Default concurrency is 4.
  live = 0; peak = 0;
  await fetchAll(ids, async () => { live++; peak = Math.max(peak, live); await sleep(5); live--; });
  assert.strictEqual(peak, 4, `default peak ${peak}`);
  // The first error rejects, and nothing new starts after it.
  const calls = [];
  await assert.rejects(
    fetchAll(ids, async (i) => {
      calls.push(i);
      await sleep(10);
      if (i === 1) throw new Error("boom 1");
      return i;
    }, { concurrency: 2 }),
    /boom 1/
  );
  await sleep(60);
  assert.ok(calls.length <= 3, `started ${calls.length} calls after the failure`);
  console.log("hidden ok");
})().catch((e) => { console.error(e); process.exit(1); });
