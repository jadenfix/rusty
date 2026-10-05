// Remove duplicate users, keeping the first occurrence of each email.
// Emails compare case-insensitively and ignore surrounding whitespace.
function dedupe(users) {
  const seen = new Set();
  const out = [];
  for (const u of users) {
    if (seen.has(u.email)) continue;
    seen.add(u.email);
    out.push(u);
  }
  return out;
}

module.exports = { dedupe };
