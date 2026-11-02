; ============================================================
; EasySSH v4 单文件引导器（v4.0.8 统一双模式）
;
; 双击（全新安装）：
;   - 若同目录 EasySSH\EasySSH.exe 已存在 → 直接启动（秒开，不解压）
;   - 否则 → 释放完整程序到 <exe目录>\EasySSH\ → 建桌面快捷方式 → 启动
;
; 在线升级模式（由升级器带 --upgraded 参数启动）：
;   - 在原目录覆盖安装：运行中的旧 exe 改名为 .old → 释放新 EasySSH.exe 与
;     WebView2Loader.dll → 启动新版（config.json/config.key/keys 原地保留）
;   - 兼容 3.x(Electron) 用户原地升级：覆盖 Electron 目录中的 EasySSH.exe 并
;     补充 WebView2Loader.dll，配置与已存密码无缝沿用
;
; 旧版备份(.old/.bak) 由应用下次启动时自动清理。
; ============================================================
Unicode true
ManifestDPIAware true
Name "EasySSH 安装程序"
Caption "EasySSH 安装程序"
BrandingText "EasySSH · https://easyssh.cn"
Icon "${ICON}"
OutFile "${OUTFILE}"
SetCompressor /SOLID lzma
RequestExecutionLevel user
CRCCheck on
AutoCloseWindow true
!include "LogicLib.nsh"
!include "FileFunc.nsh"

VIProductVersion "${VER4}"
VIAddVersionKey ProductName "EasySSH"
VIAddVersionKey FileDescription "EasySSH 安装程序"
VIAddVersionKey LegalCopyright "EasySSHCN"
VIAddVersionKey CompanyName "EasySSHCN"
VIAddVersionKey ProductVersion "${VERSION}"
VIAddVersionKey FileVersion "${VER4}"
VIAddVersionKey InternalName "EasySSH Setup"
VIAddVersionKey OriginalFilename "EasySSH-${VERSION}-setup.exe"

Function .onInit
  ; v4.1.7:统一平铺部署到引导器所在目录(当前目录)；总是覆盖释放(先改名旧 exe)
  StrCpy $INSTDIR "$EXEDIR"
FunctionEnd

Section "-bootstrap"
  DetailPrint "正在安装/升级 EasySSH 到 $INSTDIR ..."
  SetOutPath "$INSTDIR"

  ; ---- 第 1 步:腾出 EasySSH.exe（改名旧文件；被占用则提示关闭后重试） ----
  occupy_retry:
    ClearErrors
    IfFileExists "$INSTDIR\EasySSH.exe" 0 occupy_done
    Rename "$INSTDIR\EasySSH.exe" "$INSTDIR\EasySSH.exe.old"
    IfErrors 0 occupy_done
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION \
      "检测到 EasySSH 正在运行，无法覆盖安装。\n\n请先完全退出 EasySSH（关闭主窗口），然后点击「重试」。"
    Goto occupy_retry
  occupy_done:

  ; ---- 第 2 步:写入新版程序（被安全软件临时锁定时给出明确指引并可重试） ----
  SetOverwrite try
  write_exe_retry:
    ClearErrors
    File "/oname=EasySSH.exe" "${APP_EXE}"
    IfErrors 0 write_exe_ok
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION \
      "写入 EasySSH.exe 失败。\n\n常见原因：安全软件(如 360)正在扫描新文件。请稍等几秒后点击「重试」；若多次失败，可暂时退出安全软件后再试。"
    Goto write_exe_retry
  write_exe_ok:
    SetOverwrite try
  write_dll_retry:
    ClearErrors
    File "/oname=WebView2Loader.dll" "${LOADER_DLL}"
    IfErrors 0 write_dll_ok
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION \
      "写入 WebView2Loader.dll 失败。请稍等几秒后点击「重试」。"
    Goto write_dll_retry
  write_dll_ok:
    SetOverwrite on

  DetailPrint "创建桌面快捷方式..."
  CreateShortCut "$DESKTOP\EasySSH.lnk" "$INSTDIR\EasySSH.exe"
  DetailPrint "安装完成，正在启动 EasySSH..."
  Exec "$INSTDIR\EasySSH.exe"
SectionEnd
