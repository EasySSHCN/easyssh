<?php exit(0); ?>
{
  "_说明": "keys.example.php —— 模板文件（EasySSH site/api/gateway/）。真实 keys.php 只存在于服务器上的 /api/gateway/keys.php，绝不入库/入发布包（脱敏门禁覆盖）。gateway_url 为 easyssh-gateway 网关服务端内网回环地址（含协议，不带末尾斜杠）；gateway_admin_key 与 easyssh-gateway config.toml [admin].admin_key 完全一致；issue_path / revoke_path 为网关用户侧 token 管理端点路径；gateway_host / gateway_port 为客户端填写用对外展示地址。",
  "gateway_url": "http://127.0.0.1:8443",
  "gateway_admin_key": "REPLACE_WITH_RANDOM_ADMIN_KEY",
  "issue_path": "/admin/issue",
  "revoke_path": "/admin/revoke",
  "gateway_host": "easyssh.cn",
  "gateway_port": 443
}
