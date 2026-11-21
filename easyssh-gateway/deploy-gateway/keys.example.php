<?php
// keys.example.php —— 官网后台网关配置**模板**（说明参考）。
// 真实 keys.php 绝不提交仓库：复制本文件为 keys.php，填入真实值，
// 权限 600，属主 www。格式：自保护 PHP 头 + JSON 正文。

<?php exit(0); ?>
{
    "gateway_url": "http://127.0.0.1:8443",
    "gateway_admin_key": "<openssl rand -hex 32 的产物>"
}

/*
 * 使用说明：
 * - gateway_url：官网服务器（同机）访问网关的回环地址，网关绑定 127.0.0.1:8443。
 * - gateway_admin_key：与网关 config.toml [admin].admin_key 一致；
 *   官网后台以 X-Admin-Key 头调用 GET /admin/audit?limit=N 拉取审计（最新在前，
 *   limit 钳制 1..=500），或 POST /admin/issue 代签短期 token。
 * - 该文件承载密钥，严禁入库、严禁反代目录可下载。
 */
