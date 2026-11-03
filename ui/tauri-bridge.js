/* EasySSH v4 — Tauri IPC 兼容桥（替代 3.x preload.js 的 window.easyssh）
 * 必须在 renderer.js 之前加载（依赖 window.__TAURI__ 全局，tauri.conf withGlobalTauri:true）。
 * 语义与 3.x 完全一致，渲染端 renderer.js 无需感知后端已换成 Rust。 */
(function () {
  'use strict';
  // v4.0.5 修复：拦截 F5 / Ctrl+R 浏览器刷新（WebView2 加速键），防止整页重载导致会话断开
  window.addEventListener('keydown', (e) => {
    if (e.key === 'F5' || (e.ctrlKey && !e.shiftKey && !e.altKey && (e.key === 'r' || e.key === 'R'))) {
      e.preventDefault();
      e.stopPropagation();
    }
  }, true);

  // v4.1.2：替代 WebView2 不支持的 window.prompt / window.confirm（自绘模态，Promise 化）
  window.easysshUi = {
    prompt(title, defval) {
      return new Promise((resolve) => {
        const modal = document.getElementById('uiAskModal');
        const msg = document.getElementById('uiAskMsg');
        const row = document.getElementById('uiAskInputRow');
        const input = document.getElementById('uiAskInput');
        const ok = document.getElementById('uiAskOk');
        const cancel = document.getElementById('uiAskCancel');
        const cx = document.getElementById('uiAskCancelX');
        document.getElementById('uiAskTitle').textContent = title || '输入';
        msg.textContent = '';
        row.style.display = 'flex';
        input.value = defval || '';
        modal.classList.remove('hidden');
        setTimeout(() => input.focus(), 50);
        const done = (val) => {
          modal.classList.add('hidden');
          ok.removeEventListener('click', onOk);
          cancel.removeEventListener('click', onCancel);
          cx.removeEventListener('click', onCancel);
          document.removeEventListener('keydown', onKey, true);
          resolve(val);
        };
        const onOk = () => done(input.value);
        const onCancel = () => done(null);
        const onKey = (e) => {
          if (e.key === 'Enter') { e.preventDefault(); onOk(); }
          if (e.key === 'Escape') { e.preventDefault(); onCancel(); }
        };
        document.addEventListener('keydown', onKey, true);
        ok.addEventListener('click', onOk);
        cancel.addEventListener('click', onCancel);
        cx.addEventListener('click', onCancel);
      });
    },
    confirm(title) {
      return new Promise((resolve) => {
        const modal = document.getElementById('uiAskModal');
        const msg = document.getElementById('uiAskMsg');
        const row = document.getElementById('uiAskInputRow');
        const ok = document.getElementById('uiAskOk');
        const cancel = document.getElementById('uiAskCancel');
        const cx = document.getElementById('uiAskCancelX');
        document.getElementById('uiAskTitle').textContent = '确认';
        msg.textContent = title || '确认执行此操作？';
        row.style.display = 'none';
        ok.textContent = '确定';
        cancel.textContent = '取消';
        modal.classList.remove('hidden');
        setTimeout(() => ok.focus(), 50);
        const done = (val) => {
          modal.classList.add('hidden');
          ok.removeEventListener('click', onOk);
          cancel.removeEventListener('click', onCancel);
          cx.removeEventListener('click', onCancel);
          document.removeEventListener('keydown', onKey, true);
          ok.textContent = '确定'; cancel.textContent = '取消';
          resolve(val);
        };
        const onOk = () => done(true);
        const onCancel = () => done(false);
        const onKey = (e) => {
          if (e.key === 'Enter') { e.preventDefault(); onOk(); }
          if (e.key === 'Escape') { e.preventDefault(); onCancel(); }
        };
        document.addEventListener('keydown', onKey, true);
        ok.addEventListener('click', onOk);
        cancel.addEventListener('click', onCancel);
        cx.addEventListener('click', onCancel);
      });
    },
  };
  if (!window.__TAURI__) {
    console.error('Tauri 全局 API 未注入(withGlobalTauri)');
    window.easyssh = {};
    return;
  }
  const { invoke } = window.__TAURI__.core;
  const { listen } = window.__TAURI__.event;

  // 设置缓存（config.json 的 settings 与 sessions 一起落盘）
  let cachedSettings = {};

  // 版本号预取（3.x getVersionSync 为同步语义；异步预取后近似同步）
  let cachedVersion = '';
  invoke('get_version')
    .then((v) => { cachedVersion = v; })
    .catch(() => {});

  window.easyssh = {
    getVersionSync: () => cachedVersion,
    getVersion: () => invoke('get_version'),

    /* ---------- 会话/设置 ---------- */
    loadSessions: async () => {
      const cfg = await invoke('load_config');
      cachedSettings = cfg.settings || {};
      return cfg.sessions || [];
    },
    saveSessions: (list) => invoke('save_config', { sessions: list, settings: cachedSettings }),
    sessionsExport: async (defaultName, data) => {
      const path = await invoke('pick_save', { name: defaultName || 'easyssh-sessions.json' });
      if (!path) return null;
      await invoke('save_text_file', { path, data });
      return path;
    },
    sessionsImport: async () => {
      const path = await invoke('pick_open');
      if (!path) return null;
      return await invoke('read_text_file', { path });
    },

    /* ---------- SSH ---------- */
    connect: async (opts) => {
      try {
        const res = await invoke('ssh_connect', { opts });
        return { ok: true, id: res.id, info: '已连接: ' + (opts.host || ''), fingerprint: res.fingerprint };
      } catch (e) {
        const msg = String(e && e.message || e);
        if (msg.startsWith('FINGERPRINT_MISMATCH:')) {
          // FINGERPRINT_MISMATCH:<实际指纹>:<已存指纹>
          const rest = msg.slice('FINGERPRINT_MISMATCH:'.length);
          const idx = rest.indexOf(':');
          return {
            ok: false,
            mismatch: true,
            fingerprint: rest.slice(0, idx),
            stored: rest.slice(idx + 1),
            info: '服务器指纹不匹配',
          };
        }
        return { ok: false, info: msg };
      }
    },
    write: (id, data) => invoke('ssh_write', { id, data }).catch(() => {}),
    resize: (id, cols, rows) => invoke('ssh_resize', { id, cols, rows }).catch(() => {}),
    disconnect: (id) => invoke('ssh_disconnect', { id }).catch(() => {}),
    onData: (cb) => listen('ssh-data', (ev) => cb(ev.payload.id, ev.payload.data)),
    onClosed: (cb) => listen('ssh-closed', (ev) => {
      // v4.3.0:会话断开联动失效其 SFTP 通道缓存
      window.easyssh.__sftpCache.delete(ev.payload.id);
      cb(ev.payload.id);
    }),

    /* ---------- 设置 ---------- */
    loadSettings: async () => {
      const cfg = await invoke('load_config');
      cachedSettings = cfg.settings || {};
      return cachedSettings;
    },
    saveSettings: (s) => {
      cachedSettings = s || {};
      return invoke('save_config', { sessions: window.easyssh.__sessionsCache || [], settings: s });
    },
    // renderer 保存设置前通常刚 load 过；为保险，桥里维护最新 sessions 快照
    __setSessionsCache: (list) => { window.easyssh.__sessionsCache = list; },

    /* ---------- SFTP（按会话懒打开 SFTP 通道） ---------- */
    __sftpCache: new Map(),
    __ensureSftp: async (id) => {
      if (window.easyssh.__sftpCache.has(id)) return window.easyssh.__sftpCache.get(id);
      const sftpId = await invoke('sftp_open', { id });
      window.easyssh.__sftpCache.set(id, sftpId);
      return sftpId;
    },
    // v4.3.0 健壮性:通道自愈——断线重连后后端通道已释放,失败时清缓存重开一次再试
    __withSftp: async (id, op) => {
      try {
        return await op(await window.easyssh.__ensureSftp(id));
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return op(await window.easyssh.__ensureSftp(id));
        }
        throw e;
      }
    },
    sftpList: async (id, dir) => {
      return window.easyssh.__withSftp(id, (sftpId) => invoke('sftp_list', { sftpId, dir }));
    },
    pickSave: (name) => invoke('pick_save', { name }),
    pickOpen: () => invoke('pick_open'),
    sftpDownload: async (id, remote, local) => {
      return window.easyssh.__withSftp(id, (sftpId) =>
        invoke('xfer_start', { sftpId, direction: 'download', localPath: local, remotePath: remote }));
    },
    sftpUpload: async (id, local, remote) => {
      return window.easyssh.__withSftp(id, (sftpId) =>
        invoke('xfer_start', { sftpId, direction: 'upload', localPath: local, remotePath: remote }));
    },
    // v4.3.4 修复:补上漏掉的 sftpMetadata(此前未暴露,右键"属性"调用 undefined 静默失败)
    sftpMetadata: async (id, path) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      try {
        return await invoke('sftp_metadata', { sftpId, path });
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return invoke('sftp_metadata', { sftpId: await window.easyssh.__ensureSftp(id), path });
        }
        return null;
      }
    },
    sftpRename: async (id, oldPath, newPath) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      try {
        return await invoke('sftp_rename', { sftpId, oldPath, newPath });
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return invoke('sftp_rename', { sftpId: await window.easyssh.__ensureSftp(id), oldPath, newPath });
        }
        // v4.2.3 修复:Tauri Err 走 reject,渲染端 sftpRemoteOp 只认 {ok:false};不包装会静默失败
        return { ok: false, message: msg };
      }
    },
    sftpRemove: async (id, remotePath) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      try {
        // v4.2.3 修复:后端命令参数名为 path(此前传 remotePath 导致删除/新建目录 invalid args)
        return await invoke('sftp_remove_file', { sftpId, path: remotePath });
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return invoke('sftp_remove_file', { sftpId: await window.easyssh.__ensureSftp(id), path: remotePath });
        }
        return { ok: false, message: msg };
      }
    },
    sftpMkdir: async (id, dirPath) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      try {
        return await invoke('sftp_mkdir', { sftpId, path: dirPath });
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return invoke('sftp_mkdir', { sftpId: await window.easyssh.__ensureSftp(id), path: dirPath });
        }
        return { ok: false, message: msg };
      }
    },
    // v4.3.2 新增:递归计算目录占用(后端防失控:深度40/条目20万上限)
    sftpDirSize: async (id, dirPath) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      try {
        return await invoke('sftp_dirsize', { sftpId, path: dirPath });
      } catch (e) {
        const msg = String(e && e.message || e);
        if (/SFTP 未打开|会话不存在/.test(msg)) {
          window.easyssh.__sftpCache.delete(id);
          return invoke('sftp_dirsize', { sftpId: await window.easyssh.__ensureSftp(id), path: dirPath });
        }
        return { ok: false, message: msg };
      }
    },

    /* ---------- 窗口主题（v4 默认窗口装饰，占位空实现） ---------- */
    setWinTheme: () => {},

    /* ---------- 密钥 ---------- */
    generateKey: () => invoke('key_generate'),
    installKey: (id, pubLine) => invoke('key_install', { id, pubLine }),
    diagnoseKey: (id, pubLine) => invoke('key_diagnose', { id, pubLine }),

    /* ---------- 端口转发 ---------- */
    pfStart: async (id, cfg) => {
      try {
        const key = await invoke('pf_start', { id, cfg });
        return { ok: true, key };
      } catch (e) {
        return { ok: false, message: String(e && e.message || e) };
      }
    },
    pfStop: async (id, localPort) => {
      try {
        await invoke('pf_stop', { id, localPort });
        return { ok: true };
      } catch (e) {
        return { ok: false, message: String(e && e.message || e) };
      }
    },
    pfList: (id) => invoke('pf_list', { id }),
    onPfEvent: (cb) => listen('pf:event', (ev) => cb(ev.payload)),

    /* ---------- 反馈 ---------- */
    fbSubmit: (payload) => invoke('fb_submit', { payload }),

    /* ---------- 在线升级 ---------- */
    updateCheck: () => invoke('update_check'),
    updateLast: async () => window.easyssh.__updateLast || null,
    updateDownload: (entry) => invoke('update_download', { entry }),
    updateOpenFile: (file) => invoke('update_open_file', { file }),
    updateComplete: () => invoke('update_complete'),
    onUpdateEvent: (cb) => listen('update:event', (ev) => cb(ev.payload)),
    // v5(M1)/F-M1-01：签名公钥指纹（设置页展示）
    updatePubkeyFp: () => invoke('update_pubkey_fp'),
    // v5(M1)/F-M1-02：回滚提示（--rollback 启动时非空）
    rollbackNotice: () => invoke('rollback_notice'),

    /* ---------- 传输队列 ---------- */
    xferStart: async (id, cfg) => {
      const sftpId = await window.easyssh.__ensureSftp(id);
      return invoke('xfer_start', { sftpId, ...cfg });
    },
    xferCancel: (taskId) => invoke('xfer_cancel', { taskId }),
    // v5(M4):传输队列暂停/恢复
    xferPause: (taskId) => invoke('xfer_pause', { taskId }),
    xferResume: (taskId) => invoke('xfer_resume', { taskId }),
    // v5(M4):端口转发规则持久化与一键批量应用
    pfForwardsSave: (sessionKey, forwards) => invoke('pf_forwards_save', { sessionKey, forwards }),
    pfForwardsGet: (sessionKey) => invoke('pf_forwards_get', { sessionKey }),
    pfApplySession: (id, sessionKey) => invoke('pf_apply_session', { id, sessionKey }),
    // v4.2.3:WebView2 旧缓存清理机制
    cfgDirPath: () => invoke('cfg_dir_path'),
    uiCacheStatus: () => invoke('ui_cache_status'),
    uiCacheClear: () => invoke('ui_cache_clear'),
    saveTextFile: (p, d) => invoke('save_text_file', { path: p, data: d }),
    readTextFile: (p) => invoke('read_text_file', { path: p }),
    xferList: () => invoke('xfer_list'),
    onXferProgress: (cb) => listen('xfer:progress', (ev) => cb(ev.payload)),

    /* ---------- 拖拽上传（WebView2 沙箱内无法取得本地路径，走上传按钮） ---------- */
    getPathForFile: () => '',

    /* ---------- 远端文件操作 ---------- */

    /* ---------- 主密码 ---------- */
    masterpwStatus: () => invoke('masterpw_status'),
    masterpwUnlock: (pw) => invoke('masterpw_unlock', { pw }),
    masterpwEnable: (pw) => invoke('masterpw_enable', { pw }),
    masterpwChange: (oldPw, newPw) => invoke('masterpw_change', { oldPw, newPw }),
    masterpwDisable: (pw) => invoke('masterpw_disable', { pw }),
    masterpwReset: () => invoke('masterpw_reset'),

    /* ---------- 已知主机指纹 ---------- */
    knownHostsGet: (host, port) => invoke('knownhosts_get', { host, port }),
    knownHostsRecord: (host, port, fp) => invoke('knownhosts_record', { host, port, fp }),
  };

  // sessions 快照维护：包一层 loadSessions/saveSessions
  const _loadSessions = window.easyssh.loadSessions;
  window.easyssh.loadSessions = async () => {
    const list = await _loadSessions();
    window.easyssh.__sessionsCache = list;
    return list;
  };
  const _saveSessions = window.easyssh.saveSessions;
  window.easyssh.saveSessions = async (list) => {
    window.easyssh.__sessionsCache = list;
    return _saveSessions(list);
  };
})();
