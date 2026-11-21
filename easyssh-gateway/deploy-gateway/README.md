# easyssh-gateway 部署套件（deploy-gateway）—— 自建网关 + Web 服务台一站式部署

本套件把「云网关」拆成两部分，**两部分都要装**才是完整可用的云网关服务：

| 组件 | 说明 | 安装方式 |
|---|---|---|
| ① 网关服务端（二进制） | WSS 隧道 / 登录 / 策略 / 审计，绑回环 8443 | `deploy.sh`（本目录） |
| ② Web 服务台（PHP 站点组件） | 申请页 apply.html、用户服务台 gw-admin.html、api/gateway 接口、**每天 10:00/16:00 两次人工或 AI 定时审核（双结果邮件）** | `install-webconsole.sh`（本目录） |

> ①② 建议同机部署（服务台经 `http://127.0.0.1:8443` 内网回环调网关签发 token）。

## 目录内容

```
deploy-gateway/
├── easyssh-gateway(.exe)      # 网关服务端二进制（Linux 用 musl 静态构建产物，无后缀）
├── config.toml.template       # 配置模板（生产必须显式 dev_mode = false）
├── deploy.sh                  # ① 网关安装：keys.php 放置 + systemd + 健康探测
├── install-webconsole.sh      # ② Web 服务台安装：页面/接口/数据目录/定时审核 cron
├── webconsole/                # ② 的站点文件清单（由构建脚本从 EasySSH 仓库同步）
│   ├── api/gateway/{apply,audit,issue,keys.example}.php
│   ├── gw-admin.html
│   └── apply.html
├── gateway-apply-audit.php    # 定时审核任务（AI 启发式 v1 + 双结果邮件通知）
├── keys.example.php           # 审计/签发密钥模板（真件 keys.php 绝不入库）
└── README.md
```

## 安装步骤

### 前置
- Linux 服务器 + nginx + php-fpm 站点（Web 服务台宿主）；
- EasySSH 官网源码中的 `site/api/feedback/` 已在站点（fb_* 助手、邮件通道 `fb_mail_send` 的载体；
  其 `keys.php` 需配置 `"mail" => {url, key}`，否则受理/结果邮件不发——只影响通知，不影响功能）；
- 网关 `config.toml`：`dev_mode = false`（生产强制）、`[admin].admin_key` 与 keys.php 一致、
  `[[resources]]` 注册各用户的 SSH 后端、`[[policy.rules]]` 放行 `user × connect`。

### ① 网关服务端
```bash
cp config.toml.template config.toml   # 按注释填 jwt_secret / admin_key / resources / policy
# 生成 keys.php（gateway_url=http://127.0.0.1:8443，gateway_admin_key 同上）
sudo bash deploy.sh                   # keys.php 放置 + systemd 常驻 + /admin/audit 探测
```
nginx（TLS 终结）只放三条精确路由，其余一律不走网关：
```nginx
location = /api/auth/login { proxy_pass http://127.0.0.1:8443; proxy_set_header X-Real-IP $remote_addr; ... }
location = /api/resources  { proxy_pass http://127.0.0.1:8443; ... }
location ^~ /tunnel/       { proxy_pass http://127.0.0.1:8443; proxy_http_version 1.1;
                             proxy_set_header Upgrade $http_upgrade; proxy_set_header Connection "upgrade";
                             proxy_read_timeout 3600s; }
location ^~ /admin/        { return 403; }
```

### ② Web 服务台 + 定时审核
```bash
sudo bash install-webconsole.sh /www/wwwroot/easyssh.example.com
```
自动完成：接口与页面落位、数据目录（webroot 外 `<父目录>/data`）建权、
**定时审核 cron（每天 10:00 / 16:00 两次，人工或 AI 口径，通过与否均邮件通知）**、php -l 校验。

### ③ 业务流程（装完即此流程，程序级强制）
1. 用户到 `gw-admin.html` 注册账号；
2. 登录后系统**自动判定**账号邮箱的申请状态：未申请 → 强制显示登录态保护的鉴权申请页
   （邮箱自动绑定账号）；审核中/被拒各有状态页，服务台内页锁定；
3. 定时审核出结论（通过/拒绝）→ 邮件通知申请人；
4. 通过后进服务台内页「我的资源」自助签发短期 Token（约 10 分钟）→ EasySSH 客户端网关折叠区接入。

## 安全口径（首发即内置）
- `dev_mode = false`：OIDC 占位登录与账号密码 dev 通道一律 401 `auth_placeholder_denied` + 审计；
- nginx 仅三条精确路由，`/admin/*` 403；`keys.php` / `config.toml` / 数据目录永不入 webroot 版本管理；
- 导出/备份剥离网关凭据（客户端侧）；Token 短期、服务端内网签发、可吊销。

## 升级与回滚
- 网关二进制：替换 `current/easyssh-gateway` 后 `systemctl restart easyssh-gateway`
  （EasySSH 官方发布链路为「落位不自动重启」，由运维择窗口执行）；
- 回滚：保留 `releases/v<ver>/` 与 `backup/` 快照，二进制单件回退；
- Web 服务台：重跑 `install-webconsole.sh` 即幂等覆盖（keys.php 真件不会被覆盖）。

## 常见问题
- **邮件收不到**：检查 `api/feedback/keys.php` 的 `mail` 配置（fb_mail_send 通道）与服务器出站；
- **服务台登录后看不到资源**：程序性强关联——需先有已通过的云网关服务申请，且管理员已按
  账号邮箱在网关注册 `[[resources]]` 并在资源库登记授权；
- **审核没跑**：`crontab -l | grep gw-apply-audit`；日志在 `<webroot父目录>/logs/gw-audit.log`。
