// 临时冒烟:验证 sessions.js 中 R-003 解析器逻辑(与源码同步复制)
function parseSshConfig(text) {
  const out = [];
  let cur = null;
  let curName = null;
  const flush = () => {
    if (cur && cur.host && curName && !/[*?!]/.test(curName)) out.push({ ...cur, name: curName });
    cur = null; curName = null;
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^(\S+)\s*(.*)$/);
    if (!m) continue;
    const key = m[1].toLowerCase(), val = m[2].trim();
    if (key === 'host') {
      flush();
      curName = val.split(/\s+/)[0];
      cur = {};
    } else if (!cur) { continue; }
    else if (key === 'hostname') cur.host = val.split(/\s+/)[0];
    else if (key === 'user') cur.username = val.split(/\s+/)[0];
    else if (key === 'port') cur.port = Number(val) || 22;
  }
  flush();
  return out;
}
function parseTermius(data) {
  const root = (data && data.data && Array.isArray(data.data.hosts)) ? data.data : (Array.isArray(data && data.hosts) ? data : null);
  if (!root) return [];
  const creds = {};
  const credList = root.credentials || (data && data.credentials) || [];
  for (const c of credList) if (c && c.id) creds[c.id] = c.username || '';
  const out = [];
  for (const h of root.hosts) {
    if (!h) continue;
    const host = h.address || h.host || h.hostname;
    if (!host) continue;
    let username = h.username || '';
    const cids = h.credential_ids || h.credentials || [];
    if (!username && Array.isArray(cids) && cids.length) username = creds[cids[0]] || '';
    out.push({ name: h.label || h.name || '', host, port: Number(h.port) || 22, username: username || 'root' });
  }
  return out;
}
const assert = require('assert');

// ssh-config
const cfg = `
# comment
Host web-*
  User deploy
Host web1
  HostName 10.0.0.1
  User root
  Port 2222
Host db1
  hostname 10.0.0.2
`;
const r1 = parseSshConfig(cfg);
assert.deepStrictEqual(r1, [
  { host: '10.0.0.1', username: 'root', port: 2222, name: 'web1' },
  { host: '10.0.0.2', name: 'db1' },
]);
// 通配块后续键不落入
const r2 = parseSshConfig('Host *\n  User bad\nHost ok\n  HostName 1.1.1.1\n');
assert.deepStrictEqual(r2, [{ name: 'ok', host: '1.1.1.1' }]);

// termius(顶层 data.hosts)
const r3 = parseTermius({ data: { hosts: [
  { label: 'prod', address: '2.2.2.2', port: 22, credential_ids: ['c1'] },
  { label: 'nouser', hostname: '3.3.3.3' },
] }, credentials: [{ id: 'c1', username: 'ubuntu' }] });
assert.deepStrictEqual(r3, [
  { name: 'prod', host: '2.2.2.2', port: 22, username: 'ubuntu' },
  { name: 'nouser', host: '3.3.3.3', port: 22, username: 'root' },
]);
// termius(直接 hosts)
assert.strictEqual(parseTermius({ hosts: [{ address: '4.4.4.4' }] }).length, 1);
// 无效输入
assert.deepStrictEqual(parseTermius({ foo: 1 }), []);

// xshell 解析(用最小 DOMParser 模拟真实 .xsh 属性布局)
function fakeDOMParser(entries, rootAttrs) {
  const mk = (tag, attrs, children = []) => ({
    tagName: tag, attributes: Object.keys(attrs).map((k) => ({ name: k, value: attrs[k] })), children,
    getAttribute: (n) => attrs[n] != null ? attrs[n] : null, // 真实 XML DOM 大小写敏感,精确匹配
    textContent: attrs['#text'] || '',
  });
  const kids = entries.map(([n, v]) => mk('Param', { Name: n, Value: v }));
  const root = mk('Session', { name: 'MySession', ...(rootAttrs || {}) }, [mk('Params', {}, kids)]);
  return { documentElement: root, querySelector: (s) => (s === 'parsererror' ? null : null) };
}
function parseXshell(text) {
  let doc;
  try { doc = new DOMParser().parseFromString(text, 'text/xml'); } catch { return []; }
  if (doc.querySelector('parsererror')) return [];
  const vals = {};
  const put = (k, v) => { if (v && !vals[k]) vals[k] = v; };
  const walk = (el) => {
    const n = el.getAttribute && el.getAttribute('Name');
    const v = el.getAttribute && el.getAttribute('Value');
    if (n && v != null) put(n.toLowerCase(), v);
    for (const attr of el.attributes || []) {
      const k = attr.name.toLowerCase();
      if (['host', 'hostname', 'port', 'portnumber', 'user', 'username', 'loginuser'].includes(k)) put(k === 'loginuser' ? 'username' : k, attr.value);
    }
    for (const child of el.children || []) {
      const k = (child.tagName || '').toLowerCase();
      if (['host', 'hostname', 'port', 'portnumber', 'user', 'username'].includes(k) && child.textContent) put(k === 'portnumber' ? 'port' : k, child.textContent.trim());
      walk(child);
    }
  };
  walk(doc.documentElement);
  const host = vals.host || vals.hostname;
  if (!host) return [];
  return [{
    host,
    port: Number(vals.port) || 22,
    username: vals.username || vals.user || 'root',
    name: vals.name || doc.documentElement.getAttribute('name') || '',
  }];
}
globalThis.DOMParser = function () { return { parseFromString: () => fakeDOMParser([['Host', '5.5.5.5'], ['Port', '2223'], ['User', 'admin'], ['Protocol', 'SSH']]) }; };
const r4 = parseXshell('<?xml version="1.0"?><Session></Session>');
assert.deepStrictEqual(r4, [{ host: '5.5.5.5', port: 2223, username: 'admin', name: 'MySession' }]);
globalThis.DOMParser = function () { return { parseFromString: () => fakeDOMParser([['Port', '22']]) }; };
assert.deepStrictEqual(parseXshell('x'), []); // 无 Host → 空
console.log('R-003 parser smoke tests: all passed');
