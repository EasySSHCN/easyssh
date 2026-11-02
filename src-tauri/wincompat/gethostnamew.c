/* 兼容桩：旧 mingw-w64 缺 GetHostNameW；rust std(sys::net) 需要该符号。
 * 实现：kernel32!GetComputerNameExW(DnsHostname) —— 链接顺序上 kernel32 导入组
 * 位于本桩之前，符号可解析（ws2_32 则位于更早的静态组，不可引用）。 */
unsigned long GetComputerNameExW(int type, unsigned short *buf, unsigned long *len);

int GetHostNameW(unsigned short *name, int clen)
{
    unsigned long size = clen > 0 ? (unsigned long)clen : 1;
    if (GetComputerNameExW(1 /*ComputerNameDnsHostname*/, name, &size))
        return 0;
    return -1;
}


/* ==== MSVC 静态库兼容桩(WebView2LoaderStatic.lib 需要) ==== */
/* /GS Cookie 校验：mingw 链接的代码无栈保护桩，空实现即可 */
void __security_check_cookie(void) { }
/* CFG(Control Flow Guard) 分派指针：无 CFG 环境下指向空实现 */
void guard_dispatch_icall_nop(void) { }
void *__guard_dispatch_icall_fptr = (void *)&guard_dispatch_icall_nop;
