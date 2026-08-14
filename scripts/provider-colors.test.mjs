import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import convert from "color-convert";

const expectedProviders = {
  "provider-claude": "#fb923c",
  "provider-codex": "#60a5fa",
  "provider-minimax": "#a78bfa",
  "provider-agent": "#c084fc",
  "provider-pi": "#15803d",
};

function cssToken(source, name) {
  return source.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6})`, "i"))?.[1];
}

function designToken(source, name) {
  return source.match(new RegExp(`^  ${name}: "(#[0-9a-f]{6})"`, "im"))?.[1];
}

// Sharma et al. CIEDE2000, using color-convert for the installed RGB→Lab step.
export function deltaE00(hexA, hexB) {
  const [l1, a1, b1] = convert.hex.lab.raw(hexA);
  const [l2, a2, b2] = convert.hex.lab.raw(hexB);
  const rad = Math.PI / 180;
  const deg = 180 / Math.PI;
  const c1 = Math.hypot(a1, b1);
  const c2 = Math.hypot(a2, b2);
  const cBar = (c1 + c2) / 2;
  const g = (1 - Math.sqrt(cBar ** 7 / (cBar ** 7 + 25 ** 7))) / 2;
  const ap1 = (1 + g) * a1;
  const ap2 = (1 + g) * a2;
  const cp1 = Math.hypot(ap1, b1);
  const cp2 = Math.hypot(ap2, b2);
  const hp = (a, b) => (Math.atan2(b, a) * deg + 360) % 360;
  const hp1 = hp(ap1, b1);
  const hp2 = hp(ap2, b2);
  const dl = l2 - l1;
  const dc = cp2 - cp1;
  let dh = hp2 - hp1;
  if (cp1 * cp2 === 0) dh = 0;
  else if (dh > 180) dh -= 360;
  else if (dh < -180) dh += 360;
  const dH = 2 * Math.sqrt(cp1 * cp2) * Math.sin((dh * rad) / 2);
  const lBar = (l1 + l2) / 2;
  const cpBar = (cp1 + cp2) / 2;
  let hpBar = hp1 + hp2;
  if (cp1 * cp2 === 0) hpBar = hp1 + hp2;
  else if (Math.abs(hp1 - hp2) <= 180) hpBar /= 2;
  else if (hpBar < 360) hpBar = (hpBar + 360) / 2;
  else hpBar = (hpBar - 360) / 2;
  const t =
    1 -
    0.17 * Math.cos((hpBar - 30) * rad) +
    0.24 * Math.cos(2 * hpBar * rad) +
    0.32 * Math.cos((3 * hpBar + 6) * rad) -
    0.2 * Math.cos((4 * hpBar - 63) * rad);
  const sl = 1 + (0.015 * (lBar - 50) ** 2) / Math.sqrt(20 + (lBar - 50) ** 2);
  const sc = 1 + 0.045 * cpBar;
  const sh = 1 + 0.015 * cpBar * t;
  const rt =
    -2 *
    Math.sqrt(cpBar ** 7 / (cpBar ** 7 + 25 ** 7)) *
    Math.sin(60 * Math.exp(-(((hpBar - 275) / 25) ** 2)) * rad);
  return Math.sqrt(
    (dl / sl) ** 2 +
      (dc / sc) ** 2 +
      (dH / sh) ** 2 +
      rt * (dc / sc) * (dH / sh),
  );
}

test("provider hues agree and Pi stays distinct from severity green", async () => {
  const [css, design, specimen] = await Promise.all([
    readFile("src/styles/index.css", "utf8"),
    readFile("DESIGN.md", "utf8"),
    readFile(".impeccable/design.json", "utf8").then(JSON.parse),
  ]);

  for (const [name, expected] of Object.entries(expectedProviders)) {
    assert.equal(cssToken(css, name), expected, `${name} CSS token`);
    assert.equal(designToken(design, name), expected, `${name} DESIGN token`);
    assert.equal(
      specimen.extensions.colorMeta[name].canonical,
      expected,
      `${name} specimen token`,
    );
  }

  const severityGood = cssToken(css, "meter-green");
  const pi = cssToken(css, "provider-pi");
  for (const surfaceSystem of ["Graphite Stack", "Flat Polish"]) {
    assert.ok(
      deltaE00(pi, severityGood) >= 20,
      `${surfaceSystem}: Pi must stay ΔE00 >= 20 from severity green`,
    );
  }
});
