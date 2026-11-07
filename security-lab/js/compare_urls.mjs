import { chromium } from 'playwright-core';
import { mkdir, writeFile } from 'node:fs/promises';

// Comparison of WHATWG URL behavior, not of Chromium internal C++ functions.
// Parsing takes place in a blank tab; no generated URL is ever visited.
const BASE = 'https://example.test/a/b?old=1#fragment';
const curated = [
  '', '.', '..', './', '../', '../../x', '/././foo', '/foo/..',
  '/foo/%2e', '/foo/%2E', '/foo/%2e.', '/foo/.%2e', '/foo/%2e%2e/',
  '/foo/%2e%2', '/foo/%2g', '/foo/%', '/foo/%2', '/foo/%00',
  '/foo/%23', '/foo/%2f', '/foo/%5c', '/foo/%252e%252e/',
  '//example.test/', 'http://example.test/', 'http://user:pass@example.test/path',
  'https://example.test/foo/../bar', 'https://example.test/a\\b',
  'https://example.test/%2e%2e/target',
  'https://EXAMPLE.test:443/', 'https://[::1]:443/', 'file:///a/../b',
  'data:text/plain,hello', 'blob:https://example.test/abc', 'mailto:test@example.test',
  '#frag', '?q=%2e%2e', '?q=a%00b', '\t/abc\n', '/a//b',
  'foo://example.test/a\\b', 'http://127.1/', 'https://example.test/%zz',
  'https://example.test/🦊', 'https://exämple.test/a',
];
let seed = 0xA5D37B91;
function next() {
  seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
  return seed;
}
const alphabet = '.%2eE/\\?#: @\t\nabcXYZ09';
const generated = [];
for (let i = 0; i < 800; i++) {
  const len = next() % 26;
  let input = '/prefix/';
  for (let j = 0; j < len; j++) input += alphabet[next() % alphabet.length];
  generated.push(input);
}
const inputs = Array.from(new Set([...curated, ...generated]));
function inspect(raw) {
  try {
    const u = new URL(raw, BASE);
    return {
      valid: true, href: u.href, origin: u.origin,
      protocol: u.protocol, host: u.host, pathname: u.pathname,
      search: u.search, hash: u.hash,
    };
  } catch {
    return { valid: false };
  }
}
const nodeResults = inputs.map(inspect);
const browser = await chromium.launch({ channel: 'chrome', headless: true, args: ['--no-sandbox'] });
let chromeVersion = '';
let chromeResults;
try {
  chromeVersion = browser.version();
  const page = await browser.newPage();
  chromeResults = await page.evaluate(({ inputs, base }) => inputs.map((raw) => {
    try {
      const u = new URL(raw, base);
      return {
        valid: true, href: u.href, origin: u.origin,
        protocol: u.protocol, host: u.host, pathname: u.pathname,
        search: u.search, hash: u.hash,
      };
    } catch {
      return { valid: false };
    }
  }), { inputs, base: BASE });
} finally {
  await browser.close();
}
const differences = [];
for (let i = 0; i < inputs.length; i++) {
  if (JSON.stringify(nodeResults[i]) !== JSON.stringify(chromeResults[i])) {
    differences.push({
      input: inputs[i], node: nodeResults[i], chrome: chromeResults[i],
      status: 'compatibility-difference-needs-review',
      security_impact_confirmed: false,
    });
  }
}
const result = {
  method: 'node-url-vs-chrome-url-in-blank-tab',
  base: BASE, cases: inputs.length,
  nodeVersion: process.version, chromeVersion,
  differences: differences.length, samples: differences.slice(0, 50),
  scope: 'URL API behavior, not Chromium internal url_canon functions',
  confirmed_security_bugs: 0,
};
await mkdir('security-lab/results', { recursive: true });
await writeFile('security-lab/results/differential.json', JSON.stringify(result, null, 2) + '\n');
const lines = [
  '# Differential WHATWG URL test',
  '',
  '- Cases: ' + result.cases,
  '- Node version: ' + result.nodeVersion,
  '- Chrome version: ' + result.chromeVersion,
  '- Differences: ' + result.differences,
  '- Confirmed security bugs: 0',
  '',
  'No generated URL is visited. This only compares public URL API behavior.',
  'Differences are compatibility candidates, not evidence of a security vulnerability.',
  'Detailed first 50 mismatches, if any, are in differential.json.',
];
await writeFile('security-lab/results/differential.md', lines.join('\n') + '\n');
console.log(lines.join('\n'));
