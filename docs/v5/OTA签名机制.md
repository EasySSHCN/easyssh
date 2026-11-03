# EasySSH v5 OTA 升级链路 Ed25519 签名机制（F-M1-01）

> 状态：已实现（5.0.0-alpha.1）；对应手册 §3.1 F-M1-01、S-OTA-01、S-OTA-05、S-OTA-06。

## 概述

升级包（ESSHENC1：AES-256-CTR + HMAC-SHA256）之外，升级清单（update manifest JSON）
新增 `ed25519` 签名字段，由发布机私钥对清单 exe 条目关键字段签名；客户端内嵌公钥
（编译期嵌入二进制）验签，验签失败拒绝下载与安装。对称主密钥泄露不再意味着可伪造
升级包。

## 密钥

| 项 | 值 |
|---|---|
| 算法 | Ed25519（ed25519-dalek 2.x，RFC 8032） |
| 私钥 | `docs/v5/update-sign-private.key`（32 字节种子 base64，**仅发布机持有，勿入库**——`.gitignore` 已排除） |
| 公钥（base64） | `XjWJX7593VwDhOElMMHAOnDWFWy71cgBkNXv/vERGHg=` |
| 公钥指纹（SHA-256 前 16 hex） | `48fa5a439c7dc385`（官网应公示一致值；设置页详情区展示） |
| 代码位置 | 公钥常量 `src-tauri/src/updater.rs` `OTA_PUBKEY_B64`；签名/验签共用 `crates/easyssh-core/src/ota_sign.rs` |

### 轮换流程（R4 应急预案）

1. 发布机：`essh-sign --keygen <新私钥文件>` 生成新密钥对；
2. 客户端：`updater.rs` 增加 `OTA_PUBKEY_B64_NEW`，过渡期双公钥验签（任一通过即可）；
3. 全量覆盖后移除旧公钥。

## 签名对象（规范化消息，协议 `ESSH-UPDATE-MANIFEST-SIG-V1`）

对以下字段构造的规范化字符串整体签名（间接覆盖安装包 SHA-256 摘要）：

```
ESSH-UPDATE-MANIFEST-SIG-V1
latestVersion=<v>
releasedAt=<r>
file=<f>
sha256=<小写摘要>
size=<字节数>
encUrl=<u>
```

清单字段：`"ed25519": "<64 字节签名的 base64>"`。
注意：`size`/`releasedAt` 若服务端留空或为 0，同样纳入签名（填什么签什么），客户端按清单原值重建消息验证。

## 工具（发布机）

```bash
cargo build -p easyssh --bin essh-sign          # 目标目录用 CARGO_TARGET_DIR=C:\temp\v5target
essh-sign --keygen <私钥文件>                    # 生成密钥对（勿覆盖已有私钥）
essh-sign sign <清单.json> <私钥文件> [--out 输出.json]
```

## 客户端行为（src-tauri/src/update_flow.rs）

1. `do_check`：信封解密得到清单后**立即验签**；签名无效 → 报"升级包签名校验失败，已中止安装"，本轮升级中止；
2. `do_download`：下载前复查验签状态，未通过拒绝下载；
3. `complete_upgrade`：改 .bak/顶替 exe 前**兜底复查**（解密后、替换前最后防线）。

## 兼容窗口（S-OTA-06）

`updater.rs` 常量 `OTA_SIG_STRICT = false`（alpha/beta 阶段）：清单无 `ed25519` 字段时
打警告日志并放行（v4 老通道 sync.js 未集成签名的过渡）。**5.0 正式版前改为 `true` 强制。**

## ESSHENC1 格式编号

本次未修改 ESSHENC1 头部（签名挂在清单而非包体），无需在附录 D 新增编号；
`wrap` 加密格式编号亦未变更（Argon2id 沿用 `wrap:v3`，见附录 D）。

## 待服务器侧配合（sync.js）

- 清单生成后调用 `essh-sign sign` 写入 `ed25519` 字段再下发；
- 官网公示公钥指纹 `48fa5a439c7dc385`；
- 签名服务自测包纳入冒烟门禁（附录 E 用例 4、8）。
