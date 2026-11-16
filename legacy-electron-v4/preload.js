const { contextBridge, ipcRenderer, webUtils } = require('electron');

contextBridge.exposeInMainWorld('easyssh', {
  getVersionSync: () => ipcRenderer.sendSync('app:getVersion'),
  loadSessions: () => ipcRenderer.invoke('sessions:load'),
  saveSessions: (list) => ipcRenderer.invoke('sessions:save', list),
  // v2.9.0 会话导入导出(导出数据已由渲染端脱敏,不含密码/口令)
  sessionsExport: (defaultName, data) => ipcRenderer.invoke('sessions:export', defaultName, data),
  sessionsImport: () => ipcRenderer.invoke('sessions:import'),
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
  // v2.10 SFTP 强化:传输队列
  xferStart: (id, cfg) => ipcRenderer.invoke('xfer:start', id, cfg),
  xferCancel: (taskId) => ipcRenderer.invoke('xfer:cancel', taskId),
  xferList: () => ipcRenderer.invoke('xfer:list'),
  onXferProgress: (cb) => ipcRenderer.on('xfer:progress', (_e, p) => cb(p)),
  // v2.10 拖拽上传:Electron 33 渲染端拿不到 File.path,由主进程侧 webUtils 还原真实本地路径
  getPathForFile: (file) => webUtils.getPathForFile(file),
  // v2.10 远端文件操作走 SFTP 通道(不再写终端 shell 命令)
  sftpRename: (id, oldPath, newPath) => ipcRenderer.invoke('sftp:rename', id, oldPath, newPath),
  sftpRemove: (id, remotePath) => ipcRenderer.invoke('sftp:remove', id, remotePath),
  sftpMkdir: (id, dirPath) => ipcRenderer.invoke('sftp:mkdir', id, dirPath),
  // v3.0 主密码(PBKDF2 包裹 DEK)
  masterpwStatus: () => ipcRenderer.invoke('masterpw:status'),
  masterpwUnlock: (pw) => ipcRenderer.invoke('masterpw:unlock', pw),
  masterpwEnable: (pw) => ipcRenderer.invoke('masterpw:enable', pw),
  masterpwChange: (oldPw, newPw) => ipcRenderer.invoke('masterpw:change', oldPw, newPw),
  masterpwDisable: (pw) => ipcRenderer.invoke('masterpw:disable', pw),
  masterpwReset: () => ipcRenderer.invoke('masterpw:reset'),
  // v3.0 已知主机指纹
  knownHostsGet: (host, port) => ipcRenderer.invoke('knownhosts:get', host, port),
  knownHostsRecord: (host, port, fp) => ipcRenderer.invoke('knownhosts:record', host, port, fp),
});

