# v5 调研：OSC 7 / OSC 133 与终端 cwd 恢复（P2）

对应功能设想：会话重连/新建时自动恢复到上次工作目录；以及后续「命令级」UI（运行时长、退出码标记）的前置地基。
**结论先行：纯前端 term 模块即可闭环，Rust 层（ssh.rs/main.rs）无需改动。**

## 一、两个序列是什么

| 序列 | 名称 | 典型载荷 | 作用 |
|------|------|----------|------|
| OSC 7 | Current Working Directory 上报 | `ESC ] 7 ; file://host/home/user/proj BEL`（终止符也可为 ST `ESC \`） | shell 每次提示符出现前上报当前目录（iTerm2/macOS zsh 默认开启；VS Code 用同思路的 OSC 633/9;9 变体） |
| OSC 133 | Shell Integration 命令标记（FinalTerm 起源） | `A`=提示符开始、`B`=命令开始、`C`=命令输出开始、`D;exit_code`=命令结束 | 让终端把输出切成「提示符 / 命令 / 输出」三段，是 VS Code 终端 Shell Integration、wezterm、kitty shell-integration 的共同基础 |

二者都由 **shell 侧**（prompt 函数/PROMPT_COMMAND/PSConsoleHostReadLine 钩子）在提示符前主动写出，终端只负责解析。

## 二、xterm.js 接入点

### 方案 A（推荐）：`term.parser.registerOscHandler`

```ts
// xterm.js ≥5：parser.addHandler (v5.5+ 新 API，等价旧 registerOscHandler)
term.parser.registerOscHandler(7, (data) => {
  // data = "file://host/path" 或 "//host/path"（VS Code 风格）
  const m = /^file:\/\/([^/]+)(\/.*)$/.exec(data);
  if (m && m[1] === 'localhost' || m && !data.startsWith('file://')) {
    store.setCwd(sessionId, decodeURIComponent(m ? m[2] : data));
  }
  return false; // 返回 false：不吞序列，其他监听者（如 VS Code 集成）仍可见
});
term.parser.registerOscHandler(133, (data) => {
  // data 形如 "A" / "B" / "C" / "D;0"
  const [kind, exitCode] = data.split(';');
  markCommandBoundary(sessionId, kind, exitCode === undefined ? undefined : Number(exitCode));
  return false;
});
```

要点：
- **必须用 parser handler 而不是拦截 `onData`**：OSC/ST/BEL 的分帧、超长截断、跨 chunk 拼接已由 xterm.js 解析器处理；`onData` 拿到的是裸字节流，自己扫 `ESC ]7;…BEL` 会踩到「序列跨 websocket 分片到达」的坑。
- `registerOscHandler` 返回 `false` 表示"未消费"，链条继续传给后续 handler 与默认处理（安全默认）。
- OSC 7 的 host 段可能是 `%h` 之外的转义（老 iTerm2 用 base64 编码路径，格式 `file://host/base64`）——接入时对非 UTF-8 路径做 base64 解码回退。

### 方案 B（不推荐）：`onData` 侧正则拦截
仅作兜底参考：需要自维护 OSC 状态机（缓冲到 BEL/ST），脆弱且与 WebGL 渲染、会话恢复快照（M2 已做）交织，不建议。

## 三、数据流设计（前端闭环，Rust 零改动）

```
sshd(pseudo-tty) ──russh──► Rust 侧原样透传 pty 字节流（现状，不动）
                              │
                              ▼ xterm.js term.write(data) 现有路径
              term.parser OSC handler（新增，term 模块内）
                              │
                              ▼
          cwdStore：Map<sessionKey, { cwd, updatedAt, exitCode? }>
                              │
              ├─► 会话恢复（重连/新建会话时把 store.cwd 作为默认路径注入）
              ├─► SFTP/传输对话框默认目录联动（打开即定位当前 cwd）
              └─►（可选）状态栏显示 cwd；OSC 133 退出码 → 命令成功/失败角标
```

- cwdStore 持久化复用现有前端 localStorage 会话数据（或随会话记录一起保存），重启后仍可恢复「上次目录」。
- 事件协议不变：不新增 Tauri command、不新增 emit 事件。

### shell 侧接入（用户提示/内置开关）
- **bash/zsh**：`PROMPT_COMMAND`/`precmd` 里 echo `\e]7;file://$HOST$PWD\a` + `\e]133;A\a…`；可在「终端设置」提供一键复制配置片段。
- **PowerShell/Windows**：`prompt` 函数或 `PSConsoleHostReadLine` 输出 OSC 633（VS Code 变体）或标准 7/133；Windows Terminal 生态已有成熟片段可借鉴。
- **开箱程度**：macOS zsh 默认发 OSC 7（无需配置）；其余 shell 需要用户装 integration 片段 → UI 上按「已接入/未接入」提示。

## 四、风险

1. **依赖 shell 侧配合**：终端无法凭空知道远端 cwd；未装 integration 片段的会话 cwd 恒为空 → 功能必须设计为「有则恢复、无则静默」，不可作为硬依赖。
2. **序列冲突/多消费者**：VS Code 用 OSC 633 而非 7/133（语义重叠）；若 EasySSH 用户同时用 VS Code 片段，建议同时监听 633 的 `A/B/D/Cwd` 子类型（handler 链返回 false 不互斥，成本低）。
3. **不可信输入**：远端 shell 上报的路径是外部数据——存库前做长度/字符白名单校验，且只用作「默认路径建议」，永不自动执行任何命令。
4. **host 判定**：跳板链/代理下 OSC 7 的 host 段可能与会话 host 不一致（跳板机的 hostname），恢复 cwd 时只按 sessionId 绑定、不按 host 匹配，避免串目录。
5. **性能**：OSC handler 在 write 热路径上，回调须同步轻量（Map 写入即可）；OSC 133 每个提示符一次，量级无忧。

## 五、工作量估计

| 项 | 估计 |
|----|------|
| OSC 7/133 parser handler + cwdStore（前端 term 模块） | 0.5~1 天 |
| 会话恢复/SFTP 对话框联动 + 状态栏展示 | 0.5 天 |
| shell integration 片段（bash/zsh/PowerShell）与设置页引导 | 0.5~1 天 |
| 单测（序列解析状态机，纯前端）+ 联调 | 0.5 天 |
| **合计** | **2~3 天（前端单点，Rust 零改动）** |

## 六、验收建议（对应手册 Given/When/Then 风格）

- Given shell 已装 OSC 7 片段，When 用户 `cd /var/log` 后断开重连，Then 新会话提示符目录恢复为 `/var/log`（SFTP 默认目录同步）。
- Given shell 未接入，When 重连，Then 行为与现状完全一致（无回归）。
- Given 命令 `false`（退出码 1），When OSC 133 `D;1` 到达，Then UI 可标记该命令失败（P2 后续扩展点）。
