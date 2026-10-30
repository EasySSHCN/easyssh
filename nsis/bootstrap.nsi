; ============================================================
; EasySSH 单文件引导器（v3.0.4 起替代旧 portable 自解压常驻模式）
;
; 双击后的行为：
;   1) 若 exe 同目录下 EasySSH\EasySSH.exe 已存在 → 直接启动它并退出（秒开，不解压）
;   2) 否则（首次运行）→ 解压完整应用到 <exe目录>\EasySSH\ → 创建桌面快捷方式 → 启动
;
; 此后用户日常通过桌面快捷方式直接运行文件夹版 EasySSH.exe：
;   - 每次启动秒开（不再每次解压 ~200MB 到 %TEMP%）
;   - 进程与文件同目录，在线升级走与 zip 文件夹版完全相同的统一逻辑，绝不涉及 temp
;
; 构建：scripts/build-bootstrap.sh（makensis 来自 electron-builder 缓存，Windows 目标）
; ============================================================
Unicode true
ManifestDPIAware true
Name "EasySSH"
Icon "${ICON}"
OutFile "${OUTFILE}"
SetCompressor /SOLID lzma
SilentInstall silent
RequestExecutionLevel user
CRCCheck on
AutoCloseWindow true
!include "LogicLib.nsh"

Var NEED_DEPLOY

Function .onInit
  StrCpy $NEED_DEPLOY "1"
  ; 安装目录固定为 exe 同目录的 EasySSH 子目录（绿色便携，不写注册表、不需要管理员）
  StrCpy $INSTDIR "$EXEDIR\EasySSH"
  IfFileExists "$INSTDIR\EasySSH.exe" 0 +2
    StrCpy $NEED_DEPLOY "0"
FunctionEnd

Section "-bootstrap"
  ${If} $NEED_DEPLOY == "1"
    SetOutPath "$INSTDIR"
    File /r "${APP_DIR}\*.*"
    ; 桌面快捷方式指向解压后的真实 exe（后续升级原地替换该文件名，快捷方式无缝沿用）
    CreateShortCut "$DESKTOP\EasySSH.lnk" "$INSTDIR\EasySSH.exe"
  ${EndIf}
  Exec "$INSTDIR\EasySSH.exe"
SectionEnd
