/* EasySSH vendor-libfko 最小 config.h（R-20261122-002）
 * 来源：fwknop 2.6.10 (Debian source package / WSL /tmp/fwknop-2.6.10，上游
 * github.com/mrsteyn/libfwknop 2.6.10 tag)。原生构建由 autoconf 生成 config.h，
 * 此处提供 MSVC 静态编译所需的最小宏集合（等价 fwknop win32/ 工程配置）：
 * 只声明标准头存在；GPGME/fiu/cunit/unistd/sys-time 全部关闭，走源内 WIN32 分支。
 */
#define STDC_HEADERS 1
#define HAVE_STRING_H 1
#define HAVE_STDLIB_H 1
#define HAVE_CTYPE_H 1
#define HAVE_STDINT_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_SYS_STAT_H 1
