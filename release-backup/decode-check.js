const fs = require('fs');
const uc = require('/opt/dsh/workspace/easyssh-site/tools/update-crypto');
const j = JSON.parse(fs.readFileSync('/www/wwwroot/easyssh.cn/api/update/check', 'utf8'));
const mk = Buffer.from(JSON.parse(fs.readFileSync('/opt/dsh/workspace/easyssh-site/tools/update-keys.json', 'utf8')).masterKeyBase64, 'base64');
const d = uc.deriveCheckKeys(mk);
const keys = d.keys || d;
try {
  const m = uc.openEnvelope(j, mk, Number(j.ts));
  console.log('ENVELOPE-OK:', JSON.stringify(m).slice(0, 400));
} catch (e) {
  console.log('OPEN-FAIL:', e.message);
  try { const m2 = uc.openEnvelope(j, d); console.log('ENVELOPE-OK2:', JSON.stringify(m2).slice(0, 400)); } catch (e2) { console.log('OPEN-FAIL2:', e2.message); }
}
