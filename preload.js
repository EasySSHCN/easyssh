const { contextBridge, ipcRenderer } = require('electron');

contextBridge.exposeInMainWorld('easyssh', {
  getVersionSync: () => ipcRenderer.sendSync('app:getVersion'),
  loadSessions: () => ipcRenderer.invoke('sessions:load'),
  saveSessions: (list) => ipcRenderer.invoke('sessions:save', list),
  connect: (opts) => ipcRenderer.invoke('ssh:connect', opts),
  write: (id, data) => ipcRenderer.send('ssh:write', id, data),
  resize: (id, cols, rows) => ipcRenderer.send('ssh:resize', id, cols, rows),
  disconnect: (id) => ipcRenderer.send('ssh:disconnect', id),
  onData: (cb) => ipcRenderer.on('ssh:data', (_e, id, d) => cb(id, d)),
  onClosed: (cb) => ipcRenderer.on('ssh:closed', (_e, id) => cb(id)),
  // v2.0 设置 / SFTP
  loadSettings: () => ipcRenderer.invoke('settings:load'),
  saveSettings: (s) => ipcRenderer.invoke('settings:save', s),
  sftpList: (id, dir) => ipcRenderer.invoke('sftp:list', id, dir),
  pickSave: (name) => ipcRenderer.invoke('sftp:pickSave', name),
  pickOpen: () => ipcRenderer.invoke('sftp:pickOpen'),
  sftpDownload: (id, remote, local, opts) => ipcRenderer.invoke('sftp:download', id, remote, local, opts),
  sftpUpload: (id, local, remote, opts) => ipcRenderer.invoke('sftp:upload', id, local, remote, opts),
  setWinTheme: (color, symbolColor) => ipcRenderer.send('win:theme', { color, symbolColor }),
  generateKey: () => ipcRenderer.invoke('key:generate'),
  installKey: (id, pubLine) => ipcRenderer.invoke('key:install', id, pubLine),
  diagnoseKey: (id, pubLine) => ipcRenderer.invoke('key:diagnose', id, pubLine),
  // v2.6 端口转发(本地转发)
  pfStart: (id, cfg) => ipcRenderer.invoke('pf:start', id, cfg),
  pfStop: (id, localPort) => ipcRenderer.invoke('pf:stop', id, localPort),
  pfList: (id) => ipcRenderer.invoke('pf:list', id),
  onPfEvent: (cb) => ipcRenderer.on('pf:event', (_e, msg) => cb(msg)),
  // v2.8.5 使用反馈(加密提交到官网)
  fbSubmit: (payload) => ipcRenderer.invoke('feedback:submit', payload),
  // v2.5 在线升级(加密检测/加密下载)
  updateCheck: () => ipcRenderer.invoke('update:check'),
  updateLast: () => ipcRenderer.invoke('update:last'),
  updateDownload: (entry) => ipcRenderer.invoke('update:download', entry),
  updateOpenFile: (file) => ipcRenderer.invoke('update:openFile', file),
  updateComplete: () => ipcRenderer.invoke('update:complete'),
  onUpdateEvent: (cb) => ipcRenderer.on('update:event', (_e, msg) => cb(msg)),
});

