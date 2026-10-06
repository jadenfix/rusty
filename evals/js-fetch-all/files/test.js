const assert = require("assert");
const { fetchAll } = require("./fetch_all");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  const out = await fetchAll([30, 10, 20], async (ms) => {
    await sleep(ms);
    return ms * 2;
  });
  assert.deepStrictEqual(out, [60, 20, 40]);
  console.log("ok");
})().catch((e) => {
  console.error(e);
  process.exit(1);
});
