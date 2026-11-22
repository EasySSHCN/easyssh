# vendor-libfko — fwknop 2.6.10 libfko vendored sources

来源（R-20261122-002）：
- 上游版本：fwknop 2.6.10（Debian source package `fwknop_2.6.10.orig.tar.gz`，
  对应上游 github.com/mrsteyn/libfwknop 的 2.6.10 tag；VERSION 文件 = 2.6.10）。
- 拷贝自：WSL2 Ubuntu-22.04 主机 `/tmp/fwknop-2.6.10/`（事先解包的源码树）。
- 许可：GPLv2+（见上游 COPYING；本目录仅随 EasySSH 源码分发，未做功能性改动）。

## 内容
- `lib/` 核心源文件（fko*.c、rijndael.c、cipher_funcs.c、digest.c、hmac.c、
  md5/sha1/sha2/sha3、base64.c）——即 libfko 全量，排除
  `fko_utests.c`（cunit 测试）与 `gpgme_funcs.c`（GPGME 可选依赖，未启用）。
- `common/strlcat.c`、`common/strlcpy.c`、`common/fko_util.c(+h)`：库内引用的
  公共工具（autoconf 构建时由 Makefile 一并编译）。
- `win32/getlogin.c(+h)`：MSVC 下的 getlogin() shim（fwknop 官方 win32 移植）。

## EasySSH 做的最小 MSVC 兼容补丁（均带 R-20261122-002 注释）
1. `config.h`：新写最小宏集（STDC_HEADERS/HAVE_STRING_H 等），替代 autoconf 产物。
2. `fko_decode.c` L589：`bzero()` → `memset()`（bzero 在 MSVC CRT 不存在）。
3. `build.rs` 编译定义：`HAVE_CONFIG_H`、`WIN32`（fwknop 源内 Win32 分支开关）、
   `SHA2_USE_MEMSET_MEMCPY`（sha2.c 走 memset 路径）、`HAVE_MEMSET`
   （fko_common.h 的 bzero 兜底宏生效）。
4. 链接 `advapi32`（getlogin.c 使用 GetUserNameA）。

## 编译入口
`src-tauri/build.rs` 用 cc crate 将上述 .c 静态编入 easyssh（fko.lib）；
Rust 侧 FFI 绑定见 `src-tauri/src/fko_ffi.rs`，SPA 封装见 `src-tauri/src/spa.rs`。

## 升级方法
从对应 fwknop 版本源码重新拷贝 lib/common/win32 文件，重放上述 1-3 补丁，
并核对 fko.h 枚举序数与 fko_ffi.rs 常量一致。
