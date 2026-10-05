const assert = require("assert");
const { dedupe } = require("./dedupe");

const users = [
  { name: "Ada", email: "ada@example.com" },
  { name: "Ada again", email: " ADA@example.com " },
  { name: "Lin", email: "lin@example.com" },
  { name: "Lin", email: "lin@example.com" },
];
const out = dedupe(users);
assert.deepStrictEqual(out.map((u) => u.name), ["Ada", "Lin"]);
assert.strictEqual(out[0].email, "ada@example.com", "original objects are returned unchanged");
console.log("ok");
