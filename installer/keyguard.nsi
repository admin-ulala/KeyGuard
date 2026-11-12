; KeyGuard 安装脚本（NSIS 3.x / Unicode）
;
; 两种构建：
;   makensis installer\keyguard.nsi                                  → 装到 Program Files，需要 UAC
;   makensis /DKEYGUARD_PER_USER installer\keyguard.nsi              → 装到 %LOCALAPPDATA%，不需要 UAC（自测用）
;
; 为什么默认 Program Files：计划任务要 `/rl highest`，只有管理员能建。
; 安装器本身是提权的，所以「装完就能自启」这件事在安装阶段一次做完，用户不用再去点 UAC。

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!define APP_NAME      "KeyGuard"
!define APP_VERSION   "0.1.12"
!define APP_PUBLISHER "KeyGuard"
!define APP_EXE       "kg-ui.exe"
!define AGENT_EXE     "kg-agent.exe"
!define TASK_NAME     "KeyGuard Agent"
!define UNINST_KEY    "Software\Microsoft\Windows\CurrentVersion\Uninstall\KeyGuard"

Name "${APP_NAME} ${APP_VERSION}"
!ifdef KEYGUARD_PER_USER
  OutFile "..\dist\KeyGuard-${APP_VERSION}-peruser-setup.exe"
!else
  OutFile "..\dist\KeyGuard-${APP_VERSION}-setup.exe"
!endif
!ifdef KEYGUARD_PER_USER
  InstallDir "$LOCALAPPDATA\KeyGuard"
  RequestExecutionLevel user
  !define REGROOT "HKCU"
  !define SHELLCTX "current"
!else
  InstallDir "$PROGRAMFILES64\KeyGuard"
  RequestExecutionLevel admin
  !define REGROOT "HKLM"
  !define SHELLCTX "all"
!endif

InstallDirRegKey ${REGROOT} "Software\KeyGuard" "InstallDir"
SetCompressor /SOLID lzma
SetCompressorDictSize 32
Icon "..\assets\keyguard.ico"
UninstallIcon "..\assets\keyguard.ico"
BrandingText "${APP_NAME} ${APP_VERSION}"
ShowInstDetails show
ShowUninstDetails show

VIProductVersion "0.1.12.0"
VIAddVersionKey "ProductName"     "${APP_NAME}"
VIAddVersionKey "FileDescription" "KeyGuard 安装程序"
VIAddVersionKey "FileVersion"     "${APP_VERSION}"
VIAddVersionKey "ProductVersion"  "${APP_VERSION}"
VIAddVersionKey "CompanyName"      "${APP_PUBLISHER}"
VIAddVersionKey "LegalCopyright"  "${APP_PUBLISHER}"

; ---------------- 界面 ----------------
!define MUI_ABORTWARNING
!define MUI_ICON "..\assets\keyguard.ico"
!define MUI_UNICON "..\assets\keyguard.ico"
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "打开 KeyGuard"
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchUi

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

; ---------------- 复用函数 ----------------

; 用 explorer 代启界面：安装器是提权的，直接 Exec 会让 kg-ui 继承 High，
; 而界面必须待在 Medium（它只通过管道跟 Agent 说话）。
Function LaunchUi
  ${If} ${FileExists} "$INSTDIR\${APP_EXE}"
    nsExec::Exec 'explorer.exe "$INSTDIR\${APP_EXE}"'
  ${EndIf}
FunctionEnd

; 建计划任务。
;
; 教训：nsExec 直接把命令行交给 CreateProcess，不经过 cmd。当安装路径带空格
; （"D:\Program Files\KeyGuard" 就是），`/tr "\"...\"" ` 这种嵌套引号会被
; Windows 的 argv 解析切碎，schtasks 只收到 `Files\KeyGuard\kg-agent.exe --agent`。
; 所以改成：先落一个 .cmd，再用 cmd.exe 执行 —— cmd 的引号规则是我们手测过的。
; 命令行、退出码、输出一律写进 task.log，静默安装时这是唯一的现场。
Function CreateTask
  StrCpy $9 "$TEMP\kg-task.cmd"
  ClearErrors
  FileOpen $3 "$9" w
  IfErrors task_done
  FileWrite $3 "@echo off$\r$\n"
  FileWrite $3 'schtasks /create /tn "${TASK_NAME}" /tr "\$\"$INSTDIR\${AGENT_EXE}\$\" --agent" /sc onlogon /rl highest /delay 0000:03 /f$\r$\n'
  FileClose $3

  nsExec::ExecToStack '"$SYSDIR\cmd.exe" /c "$9"'
  Pop $1 ; 退出码
  Pop $2 ; 输出
  CopyFiles /SILENT "$9" "$INSTDIR\task-cmd.txt"
  Delete "$9"

  ClearErrors
  FileOpen $3 "$INSTDIR\task.log" w
  IfErrors task_done
  FileWrite $3 "dir: $INSTDIR$\r$\n"
  FileWrite $3 "exit: $1$\r$\n"
  FileWrite $3 "out: $2$\r$\n"
  FileClose $3
task_done:
  Push $1
FunctionEnd

Function un.DeleteTask
  nsExec::Exec '"$SYSDIR\schtasks.exe" /delete /tn "${TASK_NAME}" /f'
FunctionEnd

; ---------------- 安装 ----------------
Section "KeyGuard（必需）" SecMain
  SectionIn RO
  SetShellVarContext ${SHELLCTX}
  SetOutPath "$INSTDIR"
  ; 覆盖安装时旧进程还咬着 exe（Agent 以 High 运行，普通 taskkill 杀不掉，这里已提权）
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${AGENT_EXE}'
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${APP_EXE}'
  Sleep 600
  File "..\target\release\${AGENT_EXE}"
  File "..\target\release\${APP_EXE}"
  File "..\assets\keyguard.ico"

  WriteRegStr ${REGROOT} "Software\KeyGuard" "InstallDir" "$INSTDIR"
  WriteRegStr ${REGROOT} "Software\KeyGuard" "Version" "${APP_VERSION}"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "Publisher" "${APP_PUBLISHER}"
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\keyguard.ico"
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "UninstallString" "$\"$INSTDIR\uninstall.exe$\""
  WriteRegStr ${REGROOT} "${UNINST_KEY}" "QuietUninstallString" "$\"$INSTDIR\uninstall.exe$\" /S"
  WriteRegDWORD ${REGROOT} "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD ${REGROOT} "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD ${REGROOT} "${UNINST_KEY}" "EstimatedSize" "$0"

  CreateDirectory "$SMPROGRAMS\KeyGuard"
  CreateShortCut "$SMPROGRAMS\KeyGuard\KeyGuard.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\keyguard.ico" 0
  CreateShortCut "$SMPROGRAMS\KeyGuard\卸载 KeyGuard.lnk" "$INSTDIR\uninstall.exe"
SectionEnd

Section "开机自启（计划任务 · 最高权限）" SecAuto
  ; 需要管理员：/rl highest 建任务只有提权进程能做。
  ; 安装器刻意排在文件落地之后，任务里写的是 $INSTDIR 的绝对路径。
  Call CreateTask
  Pop $1
  ${If} $1 != 0
    ; 非提权构建（自测）会走到这里，不该让安装失败
    DetailPrint "计划任务创建失败（退出码 $1）——非管理员构建属正常"
  ${Else}
    DetailPrint "已创建计划任务 ${TASK_NAME}"
    ; 装完立刻拉起来。不这么做，用户双击图标只会看到「Agent 未连接」，
    ; 得先注销再登录一次才有 Agent——覆盖安装时尤其明显（旧 Agent 刚被 taskkill 掉）。
    nsExec::ExecToLog '"$SYSDIR\schtasks.exe" /run /tn "${TASK_NAME}"'
    Pop $2
    Sleep 800
    DetailPrint "已启动 Agent（exit $2）"
  ${EndIf}
SectionEnd

Section "桌面快捷方式" SecDesktop
  ; 先清掉另一个上下文里的同名图标：per-user 装一次、per-machine 再装一次，
  ; 用户桌面上就会出现两个 KeyGuard（一个在 Public Desktop，一个在自己桌面）。
  SetShellVarContext all
  Delete "$DESKTOP\KeyGuard.lnk"
  SetShellVarContext current
  Delete "$DESKTOP\KeyGuard.lnk"

  ; 截图/文档里的旧名字也顺手清掉，别留一堆同款
  Delete "$DESKTOP\KeyGuard - 快捷键守卫.lnk"

  SetShellVarContext ${SHELLCTX}
  CreateShortCut "$DESKTOP\KeyGuard.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\keyguard.ico" 0
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain}    "常驻 Agent + 界面，约 1.3 MB。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecAuto}    "登录时以最高权限启动 Agent。管理员权限的游戏必须用这个方式起 Agent，否则拦不住。"
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "在桌面放一个图标。"
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------- 卸载 ----------------
Section "Uninstall"
  ; 先停进程：Agent 常驻，不杀会删不掉文件
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${AGENT_EXE}'
  nsExec::Exec '"$SYSDIR\taskkill.exe" /F /IM ${APP_EXE}'
  Sleep 400

  Call un.DeleteTask
  ; 两个上下文都清：用户可能两种安装方式都用过
  SetShellVarContext all
  Delete "$SMPROGRAMS\KeyGuard\KeyGuard.lnk"
  Delete "$SMPROGRAMS\KeyGuard\卸载 KeyGuard.lnk"
  RMDir "$SMPROGRAMS\KeyGuard"
  Delete "$DESKTOP\KeyGuard.lnk"
  SetShellVarContext current
  Delete "$SMPROGRAMS\KeyGuard\KeyGuard.lnk"
  Delete "$SMPROGRAMS\KeyGuard\卸载 KeyGuard.lnk"
  RMDir "$SMPROGRAMS\KeyGuard"
  Delete "$DESKTOP\KeyGuard.lnk"

  Delete "$INSTDIR\${AGENT_EXE}"
  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\keyguard.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey ${REGROOT} "${UNINST_KEY}"
  DeleteRegKey ${REGROOT} "Software\KeyGuard"
  ; 规则文件留着：重装后规则还在。真要清干净让用户自己删。
SectionEnd
