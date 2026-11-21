# EasySSH Gateway 安全审计记录：OIDC 占位登录洞（占位解析闸门）

- 编号：R-20261120-006 / 007 / 008 / 011（首发即内置，v5.2.3）
- 对象：`EasySSH/easyssh-gateway`（参考 easyrdp-gateway 移植级重写 + easyrdp-fix/01 设计输入）
- 日期：2026-11-21
- 结论：**通过**。占位登录默认关闭、双重闸门（dev_mode + CIDR 白名单）、真实 OIDC 主路径独立可用，回归用例全部覆盖且 `cargo test --release` 60/60 通过。

## 一、占位登录洞审计三要素

### 要素 1：dev_mode 默认值 = false（fail-safe 默认）

| 项 | 位置 | 说明 |
|---|---|---|
| 结构体字段缺省 | `src/config.rs` `AuthConfig.dev_mode: bool`（`#[serde(default)]`，bool 缺省即 false） | 配置文件缺失该字段时**不可能**误开占位登录 |
| 空配置回归 | `src/config.rs::tests::parse_empty_config_defaults`：`assert!(!cfg.auth.dev_mode)` | 空配置 dev_mode=false 显式断言 |
| 生产校验 | `deploy-gateway/deploy.sh` 第 [2/4] 步 `grep -Eq '^dev_mode\s*=\s*false'` | 部署脚本强制 config.toml 显式 `dev_mode = false`，否则拒绝部署 |
| 示例/模板口径 | `config.example.toml`、`deploy-gateway/config.toml.template` | `dev_mode = false` + 注释“生产环境必须保持 false” |

### 要素 2：CIDR 白名单闸门（dev_mode=true 也不足以放行）

| 项 | 位置 | 说明 |
|---|---|---|
| 闸门判定 | `src/auth.rs::placeholder_login_allowed`：`state.dev_mode && cidr::ip_allowed(ip, &state.dev_cidrs)` | 两个条件同时满足才放行 |
| CIDR 解析/匹配 | `src/cidr.rs`：IPv4/IPv6 前缀匹配；裸地址=/32 或 /128 | — |
| fail-closed | `cidr::ip_allowed`：IP 解析失败（"unknown"/空串）一律 false | 无来源信息（如反代未透传）不得通过 |
| fail-fast | `src/main.rs`：`parse_cidrs` 任一条非法 `process::exit(2)`；空列表缺省仅回环 127.0.0.0/8 + ::1 | 非法 CIDR 不静默放行 |
| 拒绝审计 | `src/auth.rs::login` 占位分支：401 + `auth_placeholder_denied`（error 携带 dev_mode 与来源 IP）；控制台路径同口径 `src/console.rs::console_login` | 便于发现扫描行为（easyrdp-fix/01 要求） |
| 来源 IP 获取 | `login` handler `Option<ConnectInfo<SocketAddr>>`；`console::client_ip_from_headers`（X-Real-IP / X-Forwarded-For 优先） | 网关回环绑定或反代形态均可用 |

### 要素 3：真实 OIDC code 主路径独立可用

| 项 | 位置 | 说明 |
|---|---|---|
| 占位/真实分流 | `src/auth.rs::is_placeholder_code`：code 含 `:` 判占位；否则走真实路径 | 真实授权码（IdP 签发）不含 `:` |
| 换票链路 | `do_real_oidc_exchange`：discovery（issuer 精确匹配，缓存 1h）→ token 端点表单换票（`TokenFetcher` trait，生产 `HttpIdpClient` reqwest+rustls）→ `validate_id_token`（JWKS kid 匹配，RS256/ES256，iss/aud/exp 校验）→ `extract_roles`（roles_claim→groups→["user"]） | 不受占位闸门影响；dev_mode=false 时真实 code 仍可换票（回归用例 4 证明） |
| 未配置 [oidc] | 真实 code 请求 400 + `auth_login_failed` 审计（明确失败，不静默放行） | 回归用例 `real_oidc_code_without_config_fails_clearly` |
| OTP 不变量 | `require_otp`：本地 totp_secret 用户在占位/真实两条路径均要求 6 位 TOTP（30s ±1 窗口） | 缺码/错码 401 `otp_required` |

## 二、回归用例清单（`src/tests_e2e.rs`，`cargo test --release` 60/60 通过）

| # | 用例（测试函数） | 断言 | 结果 |
|---|---|---|---|
| 1 | dev_mode=false 占位 POST 必须 401（`placeholder_login_dev_mode_false_denied_401_with_audit`） | 回环来源也 401；审计含 `auth_placeholder_denied`；响应不泄露 token | ✅ |
| 2 | dev_mode=true 白名单外拒绝（`placeholder_login_dev_mode_true_outside_allowlist_denied`） | 白名单 127.0.0.1/32，从 127.0.0.9 真实 socket 发起 → 401 + `auth_placeholder_denied`（source_ip=127.0.0.9） | ✅ |
| 3 | 白名单内占位签发但角色受限（`placeholder_login_dev_mode_true_inside_allowlist_issues_limited_token`） | 127.0.0.0/8 内 → 200，token 有效 600s，角色按占位串解析；默认策略矩阵下 user 角色 connect = Deny（策略层受限）；留 `auth_login_dev_placeholder` 审计 | ✅ |
| 4 | 真实 OIDC code 正常换票（`real_oidc_code_exchange_unaffected_by_placeholder_gate`） | dev_mode=false + 未放行占位的前提下，MockIdp（ES256 运行时生成、discovery/token/JWKS 全内联）换票 → 200，sub/roles 映射正确，`auth_login` 审计 | ✅ |
| 附 | 未配置 [oidc] 真实 code 明确失败（`real_oidc_code_without_config_fails_clearly`） | 400，提示 oidc 未配置 | ✅ |
| 附 | OTP 二次认证（`login_otp_required_then_success_with_code`） | 缺码 401 otp_required / 错码 401 / 正确码 200 | ✅ |
| 附 | RFC 6238 参考向量、TOTP 窗口容差、Base32 变体（auth 单测） | 287082 等 | ✅ |
| 附 | 隧道/资源/审计（ws echo 全链路、wss 自签全链路、401 升级拒绝、/api/resources 不泄露后端、/admin/audit 鉴权 + limit 钳制 1..=500 最新在前） | — | ✅ |

## 三、政策矩阵与审计端点（R-20261120-011 等）

- 策略矩阵：`src/policy.rs`，资源 × 角色(admin/user/readonly) × 动作(connect/list)，首条匹配生效，`*` 通配；默认 deny by default、admin 全通；mtime 30s 轮询热加载，坏配置保留旧矩阵（`policy_reload_failed` 审计）。
- 审计：`src/audit.rs` 内存环形缓冲；`GET /admin/audit?limit=N`（X-Admin-Key 恒定时间比较；未配置 key → 503；limit 钳制 1..=500，缺省 100，最新在前）；CSV 导出含 RFC4180 + 公式注入转义。
- 绑定与 TLS：默认 `127.0.0.1:8443` 回环；`[tls]` cert/key（自签即可）时 axum-server rustls 直跑 HTTPS/WSS（wss 自签回环 e2e 验证）。

## 四、遗留事项

- 控制台 `/console/login` 与 API 占位同闸门，但控制台不含 OIDC 重定向流（客户端走 `POST /api/auth/login` 真实 code 路径）。
- 审计仅内存环形缓冲；持久化（append-only 文件 / SIEM 推送）留待后续版本。
- `X-Real-IP` 信任链依赖部署形态（Nginx 反代必须覆写，不透传客户端可伪造头）；deploy.sh 生产的 nginx 配置由运维侧保证（参考 easyrdp-fix/02 形态）。
