/* ============================================================
 * state.js — 全局状态（从 renderer.js 提取）
 * 职责：终端标签表、会话列表、设置对象、版本号等全局数据的
 *       唯一持有者。其它模块只能 import 读取（ESM live binding）
 *       或调用 set* 函数改写（import 语句不允许对绑定赋值）。
 * ============================================================ */

// tab: { id, term, fit, el, name, opts, disconnected }
export const tabs = new Map();

export let activeId = null;
export function setActiveId(v) { activeId = v; }

export let sessions = [];
export function setSessions(v) { sessions = v; }

export let pendingSess = null; // v2.9.0:列表点进来的原会话条目(克隆副本区分用)
export function setPendingSess(v) { pendingSess = v; }

// 版本号从主进程实时读取(app.getVersion),避免改版时状态栏还显示旧版本号
export let ver = '';
export function setVer(v) { ver = v; }

export let appSettings = { fontFamily: 'Consolas', fontSize: 14, encoding: 'utf8', theme: 'dracula', theme2: 'dark', quickCommands: [], scrollback: 5000, copyOnSelect: false, sessionLogEnabled: false, sessionLogDir: '', lang: 'zh-CN', termRenderer: 'auto', cursorStyle: 'block', cursorBlink: false };
export function setAppSettings(v) { appSettings = v; }

/* SFTP 传输模式（二进制/文本），SFTP 面板与传输队列共用 */
export function sftpTextMode() {
  const m = document.querySelector('input[name="sftpmode"]:checked');
  return m && m.value === 'text';
}

/* v2.9.0 会话唯一键（host:port:username） */
export const sessKey = (s) => s.host + ':' + (s.port || 22) + ':' + s.username;

/* 导出/导入一律剔除的敏感字段 */
export const sessSecretFields = ['password', 'passwordEnc', 'passphrase', 'passphraseEnc'];

/* v4 补齐:默认快捷命令种子(与 3.x DEFAULT_SETTINGS 一致 + 官方 4.2.3 exe 的 TOP)。
   仅当配置里从未写入过 quickCommands(undefined)时播种;用户清空([])或自定义后完全尊重用户数据。 */
export const DEFAULT_QUICK_CMDS = [
  { name: '系统信息', cmd: 'uname -a && cat /etc/os-release 2>/dev/null | head -2' },
  { name: '磁盘占用', cmd: 'df -h' },
  { name: '内存使用', cmd: 'free -h' },
  { name: '负载/运行时长', cmd: 'uptime' },
  { name: '监听端口', cmd: 'ss -tlnp' },
  { name: '耗资源进程', cmd: 'ps aux --sort=-%cpu | head -11' },
  { name: 'TOP 监控', cmd: 'top' },
  { name: '最近登录', cmd: 'last -n 10' },
  { name: '网络连通', cmd: 'ping -c 4 223.5.5.5' },
  { name: '重启 sshd', cmd: 'systemctl restart sshd' },
  { name: '清屏', cmd: 'clear' },
];
