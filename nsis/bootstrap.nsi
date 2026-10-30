; ============================================================
; EasySSH 单文件引导器（v3.0.5 起带标准安装器外观，降低杀软误报）
;
; 双击后的行为：
;   1) 若 exe 同目录下 EasySSH\EasySSH.exe 已存在 → 直接启动它并退出（秒开，不解压）
;   2) 否则（首次运行）→ 显示安装进度窗，解压完整应用到 <exe目录>\EasySSH\ →
;      创建桌面快捷方式 → 启动
;
; 杀软误报缓解（360「修改关键程序DLL文件」类启发式）：
;   - 不再使用 SilentInstall 静默模式（静默落盘 DLL 是高危启发式特征），改为带
;     进度界面的标准安装器外观，窗口标题/品牌信息/版本资源齐全；
;   - exe 携带完整 VS_VERSIONINFO 版本资源（产品名 EasySSH、官网、版本号）；
;   - 仍为绿色便携：不写注册表、不需要管理员权限。
;
; 构建：scripts/build-bootstrap.sh（makensis 来自 electron-builder 缓存，Windows 目标）
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

; 版本资源：让杀软与系统文件属性都能识别这是 EasySSH 官方安装程序
VIProductVersion "${VER4}"
VIAddVersionKey ProductName "EasySSH"
VIAddVersionKey FileDescription "EasySSH 安装程序"
VIAddVersionKey LegalCopyright "EasySSHCN"
VIAddVersionKey CompanyName "EasySSHCN"
VIAddVersionKey ProductVersion "${VERSION}"
VIAddVersionKey FileVersion "${VER4}"
VIAddVersionKey InternalName "EasySSH Setup"
VIAddVersionKey OriginalFilename "EasySSH-${VERSION}.exe"

Var NEED_DEPLOY

Function .onInit
  StrCpy $NEED_DEPLOY "1"
  ; 安装目录固定为 exe 同目录的 EasySSH 子目录（绿色便携，不写注册表、不需要管理员）
  StrCpy $INSTDIR "$EXEDIR\EasySSH"
  IfFileExists "$INSTDIR\EasySSH.exe" 0 +2
    StrCpy $NEED_DEPLOY "0"
FunctionEnd

Function .onGUIInit
  ${If} $NEED_DEPLOY == "0"
    ; 已部署过：不打扰用户，直接进入应用（窗口一闪而过）
    ShowWindow $HWNDPARENT 0   ; SW_HIDE
  ${EndIf}
FunctionEnd

Section "-bootstrap"
  ${If} $NEED_DEPLOY == "1"
    DetailPrint "正在准备安装目录：$INSTDIR"
    SetOutPath "$INSTDIR"
    DetailPrint "正在释放 EasySSH 程序文件（约 280MB，请稍候）..."
    File /r "${APP_DIR}\*.*"
    DetailPrint "创建桌面快捷方式..."
    ; 桌面快捷方式指向解压后的真实 exe（后续升级原地替换该文件名，快捷方式无缝沿用）
    CreateShortCut "$DESKTOP\EasySSH.lnk" "$INSTDIR\EasySSH.exe"
    DetailPrint "安装完成，正在启动 EasySSH..."
  ${EndIf}
  Exec "$INSTDIR\EasySSH.exe"
SectionEnd
