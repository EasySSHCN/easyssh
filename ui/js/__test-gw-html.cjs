const fs = require('fs');
const h = fs.readFileSync('B:/Users/ZhenAn/Documents/deepseek-harness/default-workspace/EasySSH/ui/index.html', 'utf8');
const void_ = new Set(['meta','link','br','hr','img','input','use','path','circle','rect','source','col']);
const st = []; let ok = true;
const re = /<\/?([a-zA-Z][a-zA-Z0-9-]*)((?:"[^"]*"|'[^']*'|[^'">])*)>/g; let m;
while ((m = re.exec(h))) {
  const tag = m[1].toLowerCase(); const full = m[0];
  if (full.startsWith('</')) {
    const top = st.pop();
    if (top !== tag) { console.log('MISMATCH: </' + tag + '> but stack top=' + top + ' at index ' + m.index); ok = false; break; }
  } else if (!full.endsWith('/>') && !void_.has(tag)) st.push(tag);
}
if (ok && st.length) console.log('UNCLOSED:', st.join(','));
else if (ok) {
  const ids = ['gwSection','gwOtpModal','gwBackups','gwAddBackup','btnGwCheck','gwCheckMsg','gwOtpInput','gwOtpOk','gwOtpCancel','gwSkipWarn','gwTokenLabel','gwPwRows'];
  const missing = ids.filter((id) => !h.includes('id="' + id + '"'));
  console.log('HTML tag balance OK;', missing.length ? 'MISSING ids: ' + missing.join(',') : 'gateway ids all present');
}
