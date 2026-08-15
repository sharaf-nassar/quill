import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const fixtureDir = join(dirname(fileURLToPath(import.meta.url)), "fixtures/pi-usage-parity");
const manifest = JSON.parse(await readFile(join(fixtureDir, "manifest.json"), "utf8"));

const usageTotal = ({ input = 0, output = 0, cacheRead = 0, cacheWrite = 0 }) =>
  input + output + cacheRead + cacheWrite;

async function legacyUsage(name) {
  const records = (await readFile(join(fixtureDir, `${name}.jsonl`), "utf8"))
    .trim()
    .split("\n")
    .map(JSON.parse);
  return records
    .filter((record) => record.type === "message" && record.message?.role === "assistant")
    .map((record) => ({ id: record.id, tokens: usageTotal(record.message.usage ?? {}) }));
}

function pushedUsage(events) {
  return [...new Map(events.map((event) => [event.eventUuid, event])).values()].map((event) => ({
    id: event.messageId,
    tokens: usageTotal(event.usage),
  }));
}

const total = (messages) => messages.reduce((sum, message) => sum + message.tokens, 0);

// @lat: [[pi-model-usage-tests#Pi Model Usage Test Specs#Removal Parity Gate]]
test("Pi pushed usage passes the pre-removal fixture corpus", async (t) => {
  for (const fixture of manifest) {
    await t.test(fixture.name, async () => {
      const legacy = await legacyUsage(fixture.name);
      const pushed = pushedUsage(fixture.pushed);
      const legacyTotal = total(legacy);
      const pushedTotal = total(pushed);

      if (fixture.mode === "exact") {
        assert.equal(pushedTotal, legacyTotal);
        console.log(`${fixture.name}: legacy=${legacyTotal} pushed=${pushedTotal} exact`);
        return;
      }

      if (fixture.mode === "fork") {
        assert.equal(legacyTotal - pushedTotal, fixture.copiedAncestorTokens);
        assert.match(fixture.reason, /copied ancestor/);
        console.log(
          `${fixture.name}: legacy=${legacyTotal} pushed=${pushedTotal} ` +
            `delta=${legacyTotal - pushedTotal} (${fixture.reason})`,
        );
        return;
      }

      assert.equal(fixture.mode, "upgrade");
      assert.ok(fixture.pushed.length > pushed.length, "fixture must replay a pushed event");
      assert.equal(new Set([...legacy, ...pushed].map((message) => message.id)).size, legacy.length + pushed.length);
      assert.equal(legacyTotal + pushedTotal, fixture.expectedUnionTokens);
      console.log(
        `${fixture.name}: legacy=${legacyTotal} pushed=${pushedTotal} ` +
          `union=${legacyTotal + pushedTotal} replay-deduped`,
      );
    });
  }
});
