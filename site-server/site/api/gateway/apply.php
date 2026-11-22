<?php
/**
 * POST /api/gateway/apply.php —— 已退役（R-20261121-003 流程简化，对齐 EasyRDP）
 * 云网关服务申请不再作为独立环节：用户注册/登录服务台后，管理员在后台把资源授权
 * 给用户邮箱即可（grants 模式）。保留本文件返回 410，兼容历史外链/客户端探测。
 */
require __DIR__ . '/../feedback/common.php';
fb_fail('云网关服务申请流程已下线：请到云网关用户服务台 https://easyssh.cn/gw-admin.html 注册/登录，资源授权由管理员发放', 410);