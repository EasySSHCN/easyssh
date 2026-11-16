# EasySSH 官网安全加固说明（v5.1.1 / v5.1.2 / v5.1.3）

> **v5.1.3 复测加固(2026-11-11 下午,全站渗透复测)**:
> - **备份文件泄露修复**:webroot 遗留 7 个 `.bak-*` 历史文件(旧版 html/site.js)可被公网下载,已全部删除;nginx 增加 `location ~ \.bak|\.old|\.swp|\.tmp { return 404; }` 兜底
> - **安全响应头**:`X-Frame-Options: SAMEORIGIN` / `X-Content-Type-Options: nosniff` / `Referrer-Policy: strict-origin-when-cross-origin`(点击劫持与 MIME 嗅探面收敛)
> - **验证码签发频控**:`api/captcha/img.php` 每 IP 60 次/小时(滑动窗口,`data/captcha-ratelimit.json`),实测第 61 次 → 429
> - **trusted_proxies 收紧**:keys.php 移除该字段(nginx 已通过 fastcgi REMOTE_ADDR 传真实客户端 IP,本机代理信任无必要,防服务器本机伪造 XFF 换频控桶)
> - 原配置备份:`/root/sec-backup-v511/easyssh.cn.conf.bak2`
> - **CSP 已启用**(v5.1.3):`default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self'; font-src 'self'; object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'self'`——站点无外部资源,内联 script/style 保留 unsafe-inline;后续若重构资源外链可逐步收紧。无头浏览器实测页面功能(含验证码)不受影响

> **状态:已于 2026-11-11 全部部署到发布机 la.trae.show 并验证通过**(第 1–4 节)。
> 仅剩第 5 节 master 密钥轮换待与客户端发版协调(已录入 EasySSH 需求池 R-20261111-001,状态:记录)。
>
> **v5.1.2 追加(2026-11-11 下午)**:后台登录图形验证码、邮箱找回密码、管理员邮箱绑定(带邮箱验证)已部署并验证。
> - 新增 `site/api/captcha/img.php`(GD 生成,服务端一次性消费,10 分钟过期,登录强制校验)
> - `admin.php` 新增动作:profile / setemail_request / setemail_confirm(登录态)、forgot_request / forgot_reset(未登录,统一响应防枚举)
> - keys.php 新增 `admin_email`(初始为空,**首次使用需登录后台 → 📧 邮箱设置 绑定**)与 `mail`(邮件通道,gz.trae.show mail-api,Bearer key)
> - 验证码统一存 `data/verify-codes.json`:6 位数字、15 分钟有效、60 秒重发间隔、每小时最多 5 条、错 5 次作废、一次性消费
> - 服务器端邮件通道已实测可发送(fb_mail_send → ad@6xm.net 自测通过)
> - **待用户完成**:用管理员账号登录 admin.html → 邮箱设置 → 绑定真实邮箱(之后找回密码功能才可用)

## 已完成(服务器侧,全部实测验证)

1. **运行时数据移出 webroot**:PHP 路径改为 `/www/wwwroot/data/`(webroot 上级),`feedback-db.json` 已迁移,属主 `www:www 770`。`/www/wwwroot/easyssh.cn/.user.ini` 的 open_basedir 已扩展包含该目录(文件已 chattr +i)。
2. **Nginx 拦截**:`easyssh.cn.conf` 加入规则——`location = /data/site-data.json` 放行客户端依赖的公开接口;`location ~ ^/data/ { deny all; }` 拦截其余。实测:`/data/feedback-db.json` → 404,`/data/site-data.json` → 200。原配置备份在 `/root/sec-backup-v511/`。
3. **登录限速 + 可吊销会话**:15 分钟窗口失败 5 次锁定(实测第 6 次 → 429);token 改服务端随机会话(admin-sessions.json,12h 过期,最多 50 个);改用户名/密码吊销全部会话。旧格式 token 已自动失效。
4. **XFF 伪造修复 + 统计频控保护**:`fb_client_ip()` 仅信任 keys.php `trusted_proxies`(已配 `127.0.0.1,::1`)声明的代理,XFF 取最后一跳;宝塔 fastcgi 将真实客户端 IP 经 REMOTE_ADDR 传入 PHP,即使不配代理频控也用真实 IP。stats-ratelimit.json 加 10000 条上限 + 原子写。
5. **token_secret 已轮换**(服务端专用密钥,不进客户端,零影响);keys.php 新增 trusted_proxies 字段,fb_keys_save 会保留它。
6. **遗留核查结果**:`assets/js/site.js` 的 `inlineMd()` 先 escapeHtml 再加粗/代码标记,无存储型 XSS;`escapeHtml` 实现正确。

## 待办:master 密钥轮换(需与客户端发版协调)

`keys.php` 的 `master` 与升级通道 update-keys.json 同源。该文件曾随 site-pull 进入开发机工作区,若曾在任何 git 仓库/发布包/网盘出现,应轮换 master:

1. 服务器生成新 master(32 字节 base64)替换 keys.php;
2. 客户端同步更新 update-keys.json 与反馈通道派生密钥(feedback.rs / stats.rs / updater.js),随下个版本发布;
3. **旧版本客户端的反馈/统计上报会因密钥不匹配被拒**,建议保留双密钥兼容窗口或直接接受旧版本上报失败(旧版仍可手动下载)。

## 运维备忘

- `EasySSH/scripts/fb-sync.py`、`harvest-feedback.ps1` 已改用新数据路径 `/www/wwwroot/data/feedback-db.json`。
- 服务器 PHP 文件原始备份:`/root/sec-backup-v511/`(common.php×2、admin.php、keys.php、nginx conf)。
- Apache 兜底规则在 `site/.htaccess`(本站为 Nginx,不生效,仅备用)。

## 1. 运行时数据移出 webroot（高危修复）

## 待办:master 密钥轮换(需与客户端发版协调)

`keys.php` 的 `master` 与升级通道 update-keys.json 同源。该文件曾随 site-pull 进入开发机工作区,若曾在任何 git 仓库/发布包/网盘出现,应轮换 master:

1. 服务器生成新 master(32 字节 base64)替换 keys.php;
2. 客户端同步更新 update-keys.json 与反馈通道派生密钥(feedback.rs / stats.rs / updater.js),随下个版本发布;
3. **旧版本客户端的反馈/统计上报会因密钥不匹配被拒**,建议保留双密钥兼容窗口或直接接受旧版本上报失败(旧版仍可手动下载);
4. 确认 `EasySSH/site-server/` 整个目录不进任何公开仓库,公开仓库/发布包中不含 keys.php、update-keys.json、site-pull.tar.gz。

## 运维备忘

- `EasySSH/scripts/fb-sync.py`、`harvest-feedback.ps1` 已改用新数据路径 `/www/wwwroot/data/feedback-db.json`。
- 服务器 PHP/nginx 原始备份:`/root/sec-backup-v511/`。
- Apache 兜底规则在 `site/.htaccess`(本站为 Nginx,不生效,仅备用)。
- `.user.ini` 已 chattr +i,后续调整 open_basedir 需先 `chattr -i`。
