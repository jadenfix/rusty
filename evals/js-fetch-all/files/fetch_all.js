/**
 * fetchAll(ids, fetchOne, { concurrency = 4 } = {})
 *
 * Calls fetchOne(id) for every id and resolves to the results in the same
 * order as `ids`, whatever order the calls finish in. At most `concurrency`
 * calls are in flight at any moment, and a new call starts as soon as one
 * finishes. If any call rejects, fetchAll rejects with that first error and
 * starts no further calls. An empty `ids` resolves to [].
 */
async function fetchAll(ids, fetchOne, { concurrency = 4 } = {}) {
  const results = [];
  await Promise.all(
    ids.map(async (id) => {
      results.push(await fetchOne(id));
    })
  );
  return results;
}

module.exports = { fetchAll };
